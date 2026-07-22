#![allow(dead_code)]

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::domain::router::{ClientSubdomain, ShareSlug, PROTOCOL_EPOCH};
use crate::domain::settings::config::{
    PayoutProfileState, RouterIdentity, ServerConfig, UpgradePolicyConfig,
};
use crate::domain::sharing::router_contract::*;
use crate::self_update::version::LatestReleaseMeta;

const ROUTER_LEASE_RENEW_TIMEOUT: Duration = Duration::from_secs(5);
const ROUTER_TUNNEL_CONTROL_HTTP_TIMEOUT: Duration = Duration::from_secs(8);
const ROUTER_INSTALLATION_REGISTER_TIMEOUT: Duration = Duration::from_secs(10);
const ROUTER_INSTALLATION_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(10);
const ROUTER_SETUP_COMPLETED_TIMEOUT: Duration = Duration::from_secs(10);
const ROUTER_CONTROL_PLANE_SYNC_TIMEOUT: Duration = Duration::from_secs(10);
const ROUTER_INSTALLATION_REGISTER_RESPONSE_BODY_LIMIT: usize = 16 * 1024;
const ROUTER_ERROR_BODY_LIMIT: usize = 512;
const ROUTER_SHARE_PRUNE_MAX_IDS: usize = 10_000;
const REGISTRATION_PROOF_VERSION: u8 = 2;
const INSTALLATION_HEARTBEAT_PROTOCOL_VERSION: u8 = 1;
const INSTALLATION_SETUP_COMPLETED_PROTOCOL_VERSION: u8 = 1;
const INSTALLATION_SETUP_COMPLETED_ACTION: &str = "installation_setup_completed_v1";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterInstallationRequest {
    pub protocol_epoch: String,
    pub public_key: String,
    pub platform: String,
    pub app_version: String,
    pub instance_nonce: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proof_version: Option<u8>,
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

#[derive(Debug, thiserror::Error)]
pub enum RegisterInstallationAttemptError {
    #[error("router installation register request failed: {0}")]
    Request(#[source] reqwest::Error),
    #[error("build router installation register request: {0}")]
    InvalidRequest(String),
    #[error("router installation register rejected: {status}: {body}")]
    Rejected {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("parse router installation register response: {0}")]
    InvalidResponse(String),
}

impl RegisterInstallationAttemptError {
    pub fn allows_legacy_fallback(&self) -> bool {
        false
    }

    pub fn is_transient(&self) -> bool {
        match self {
            Self::Request(error) => error.is_connect() || error.is_timeout(),
            Self::Rejected { status, .. } => {
                *status == reqwest::StatusCode::REQUEST_TIMEOUT
                    || *status == reqwest::StatusCode::TOO_MANY_REQUESTS
                    || status.is_server_error()
            }
            Self::InvalidRequest(_) | Self::InvalidResponse(_) => false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InstallationHeartbeatError {
    #[error("router installation heartbeat endpoint is unavailable: {status}: {body}")]
    EndpointUnavailable {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("router installation heartbeat requires registration: {status}: {body}")]
    RegistrationRequired {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("router installation heartbeat transient failure: {0}")]
    Transient(String),
    #[error("router installation heartbeat rejected: {status}: {body}")]
    Rejected {
        status: reqwest::StatusCode,
        body: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum InstallationSetupCompletedError {
    #[error("send router installation setup-completed request: {0}")]
    Request(#[source] reqwest::Error),
    #[error("router installation setup-completed request failed: {status}: {body}")]
    Rejected {
        status: reqwest::StatusCode,
        body: String,
        retry_after_ms: Option<i64>,
    },
    #[error("parse router installation setup-completed response: {0}")]
    InvalidResponse(String),
}

impl InstallationSetupCompletedError {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Rejected {
                status: reqwest::StatusCode::BAD_REQUEST
                    | reqwest::StatusCode::FORBIDDEN
                    | reqwest::StatusCode::UNPROCESSABLE_ENTITY,
                ..
            }
        )
    }

    pub fn retry_after_ms(&self) -> Option<i64> {
        match self {
            Self::Rejected { retry_after_ms, .. } => *retry_after_ms,
            Self::Request(_) | Self::InvalidResponse(_) => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientTunnelClaimError {
    #[error("send router client tunnel claim: {0}")]
    Request(#[source] reqwest::Error),
    #[error("router client tunnel claim failed: {status}: {body}")]
    Rejected {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("router client tunnel claim timed out after {timeout_seconds}s")]
    Timeout { timeout_seconds: f64 },
}

impl ClientTunnelClaimError {
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Request(error) => error.is_connect() || error.is_timeout(),
            Self::Rejected { status, .. } => {
                *status == reqwest::StatusCode::REQUEST_TIMEOUT
                    || *status == reqwest::StatusCode::TOO_MANY_REQUESTS
                    || status.is_server_error()
            }
            Self::Timeout { .. } => true,
        }
    }
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
    pub protocol_epoch: String,
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    #[serde(flatten)]
    pub payload: T,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationHeartbeatPayload {
    pub protocol_version: u8,
    pub boot_id: String,
    pub app_version: String,
    pub commit_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_ip: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallationSetupCompletedPayload {
    pub protocol_version: u8,
    pub setup_id: String,
    pub password_hint: String,
}

impl InstallationSetupCompletedPayload {
    pub fn new(setup_id: String, password_hint: String) -> anyhow::Result<Self> {
        validate_setup_id(&setup_id)?;
        validate_password_hint(&password_hint)?;
        Ok(Self {
            protocol_version: INSTALLATION_SETUP_COMPLETED_PROTOCOL_VERSION,
            setup_id,
            password_hint,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstallationSetupCompletedEnvelope {
    setup: InstallationSetupCompletedPayload,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstallationSetupCompletedResponse {
    ok: bool,
    status: String,
    setup_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallationSetupCompletedAckStatus {
    Queued,
    AlreadyRecorded,
    SuppressedDisabled,
}

impl InstallationSetupCompletedAckStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::AlreadyRecorded => "already_recorded",
            Self::SuppressedDisabled => "suppressed_disabled",
        }
    }
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
    #[serde(default = "legacy_owner_email_is_verified")]
    pub owner_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallationOwnerEmailStatus {
    pub owner_email: Option<String>,
    pub owner_verified: bool,
}

impl From<InstallationOwnerEmailResponse> for InstallationOwnerEmailStatus {
    fn from(response: InstallationOwnerEmailResponse) -> Self {
        Self {
            owner_email: response.owner_email,
            owner_verified: response.owner_verified,
        }
    }
}

const fn legacy_owner_email_is_verified() -> bool {
    // Older Routers exposed ownerEmail only after treating the binding as verified.
    true
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientTunnelClaimRequest {
    protocol_epoch: &'static str,
    installation_id: String,
    timestamp_ms: i64,
    nonce: String,
    signature: String,
    tunnel: ClientTunnelConfig,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientTunnelUpdateRequest {
    protocol_epoch: &'static str,
    installation_id: String,
    timestamp_ms: i64,
    nonce: String,
    signature: String,
    tunnel: ClientTunnelConfig,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallationPayoutProfileUpdateRequest {
    protocol_epoch: &'static str,
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
    protocol_epoch: &'static str,
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
    protocol_epoch: &'static str,
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
    protocol_epoch: &'static str,
    installation_id: String,
    timestamp_ms: i64,
    nonce: String,
    signature: String,
    ops: Vec<ShareSyncOperation>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShareDescriptorSyncAck {
    pub share_id: String,
    pub descriptor_generation: u64,
    pub descriptor_fingerprint: String,
    #[serde(default)]
    pub applied: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareDescriptorBatchSyncResponse {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    acks: Vec<ShareDescriptorSyncAck>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareDescriptorSyncOutcome {
    Strict(Vec<ShareDescriptorSyncAck>),
    Legacy,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SharePruneRequest {
    protocol_epoch: &'static str,
    installation_id: String,
    timestamp_ms: i64,
    nonce: String,
    signature: String,
    share_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharePruneOutcome {
    Applied,
    Unsupported,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareRequestLogBatchSyncRequest {
    protocol_epoch: &'static str,
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
    protocol_epoch: &'static str,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NamespaceLeasePayload {
    pub protocol_epoch: String,
    pub router_id: String,
    pub route_id: String,
    pub rotation_id: String,
    pub generation: u64,
    pub expected_generation: u64,
    pub requested_subdomain: String,
    pub tunnel_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<ShareDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NamespaceLeaseResponse {
    pub protocol_epoch: String,
    pub router_id: String,
    pub lease_id: String,
    pub connection_id: String,
    pub route_id: String,
    pub rotation_id: String,
    pub generation: u64,
    pub expected_generation: u64,
    pub ssh_username: String,
    pub ssh_password: String,
    pub ssh_addr: String,
    pub expires_at: String,
    pub tunnel_url: String,
    pub subdomain: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_host_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NamespaceRenewLeasePayload {
    pub protocol_epoch: String,
    pub router_id: String,
    pub lease_id: String,
    pub connection_id: String,
    pub route_id: String,
    pub rotation_id: String,
    pub generation: u64,
    pub expected_generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NamespaceRenewLeaseResponse {
    pub protocol_epoch: String,
    pub router_id: String,
    pub route_id: String,
    pub rotation_id: String,
    pub generation: u64,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivateTunnelPayload {
    pub protocol_epoch: String,
    pub router_id: String,
    pub lease_id: String,
    pub connection_id: String,
    pub route_id: String,
    pub rotation_id: String,
    pub generation: u64,
    pub expected_generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TunnelStatePayload {
    pub protocol_epoch: String,
    pub router_id: String,
    pub lease_id: String,
    pub connection_id: String,
    pub route_id: String,
    pub rotation_id: String,
    pub generation: u64,
    pub expected_generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TunnelStateResponse {
    pub protocol_epoch: String,
    pub router_id: String,
    pub route_id: String,
    pub rotation_id: String,
    pub generation: u64,
    pub expected_generation: u64,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidate_generations: Vec<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub draining_generations: Vec<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelSignedRequest<T> {
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    #[serde(flatten)]
    pub payload: T,
}

pub fn tunnel_router_id(config: &ServerConfig) -> anyhow::Result<String> {
    if let Some(domain) = config
        .router
        .domain
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(domain.trim_end_matches('.').to_ascii_lowercase());
    }
    let api_base = config
        .router_api_base()
        .ok_or_else(|| anyhow::anyhow!("router api base is not configured"))?;
    reqwest::Url::parse(api_base)
        .context("parse router api base")?
        .host_str()
        .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
        .ok_or_else(|| anyhow::anyhow!("router api base does not contain a host"))
}

fn tunnel_signed_request<T: Serialize + Clone>(
    identity: &RouterIdentity,
    action: &str,
    payload: T,
) -> anyhow::Result<TunnelSignedRequest<T>> {
    let timestamp_ms = now_ms();
    let nonce = nonce();
    let signature = sign_payload(identity, action, &payload, timestamp_ms, &nonce)?;
    Ok(TunnelSignedRequest {
        installation_id: identity.installation_id.clone(),
        timestamp_ms,
        nonce,
        signature,
        payload,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterRegisterResult {
    pub installation_id: String,
    pub public_key: String,
    pub control_secret_present: bool,
    pub registered_at_ms: i64,
}

pub async fn register_installation_v2(
    http: &reqwest::Client,
    api_base: &str,
    identity: &RouterIdentity,
) -> Result<RegisterInstallationResponse, RegisterInstallationAttemptError> {
    let request = build_register_installation_request(
        identity,
        std::env::consts::OS,
        crate::build_info::router_registration_version(),
        nonce(),
        now_ms(),
    )
    .map_err(|error| RegisterInstallationAttemptError::InvalidRequest(error.to_string()))?;
    send_register_installation_request(http, api_base, &request).await
}

pub async fn discover_legacy_installation(
    http: &reqwest::Client,
    api_base: &str,
    identity: &RouterIdentity,
) -> Result<RegisterInstallationResponse, RegisterInstallationAttemptError> {
    let request = build_unsigned_legacy_register_installation_request(
        identity,
        std::env::consts::OS,
        crate::build_info::router_registration_version(),
        nonce(),
    );
    send_register_installation_request(http, api_base, &request).await
}

pub async fn recover_legacy_installation(
    http: &reqwest::Client,
    api_base: &str,
    identity: &RouterIdentity,
) -> Result<RegisterInstallationResponse, RegisterInstallationAttemptError> {
    let request = build_legacy_register_installation_request(
        identity,
        std::env::consts::OS,
        crate::build_info::router_registration_version(),
        nonce(),
        now_ms(),
    )
    .map_err(|error| RegisterInstallationAttemptError::InvalidRequest(error.to_string()))?;
    send_register_installation_request(http, api_base, &request).await
}

async fn send_register_installation_request(
    http: &reqwest::Client,
    api_base: &str,
    request: &RegisterInstallationRequest,
) -> Result<RegisterInstallationResponse, RegisterInstallationAttemptError> {
    send_register_installation_request_with_timeout(
        http,
        api_base,
        request,
        ROUTER_INSTALLATION_REGISTER_TIMEOUT,
    )
    .await
}

async fn send_register_installation_request_with_timeout(
    http: &reqwest::Client,
    api_base: &str,
    request: &RegisterInstallationRequest,
    timeout: Duration,
) -> Result<RegisterInstallationResponse, RegisterInstallationAttemptError> {
    let response = http
        .post(format!(
            "{}/v1/installations/register",
            api_base.trim_end_matches('/')
        ))
        .json(request)
        .timeout(timeout)
        .send()
        .await
        .map_err(RegisterInstallationAttemptError::Request)?;
    let status = response.status();
    if !status.is_success() {
        let body = read_bounded_router_error_body(response)
            .await
            .map_err(RegisterInstallationAttemptError::Request)?;
        return Err(RegisterInstallationAttemptError::Rejected { status, body });
    }
    let body = read_bounded_router_registration_response_body(response)
        .await
        .map_err(RegisterInstallationAttemptError::Request)?;
    if body.truncated {
        return Err(RegisterInstallationAttemptError::InvalidResponse(format!(
            "router response body exceeds the {ROUTER_INSTALLATION_REGISTER_RESPONSE_BODY_LIMIT} byte limit"
        )));
    }
    serde_json::from_slice(&body.bytes)
        .map_err(|error| RegisterInstallationAttemptError::InvalidResponse(error.to_string()))
}

pub async fn send_installation_heartbeat(
    http: &reqwest::Client,
    config: &ServerConfig,
    boot_id: &str,
    public_ip: Option<&str>,
) -> Result<(), InstallationHeartbeatError> {
    let api_base = config
        .router_api_base()
        .ok_or_else(|| {
            InstallationHeartbeatError::Transient("router api base is not configured".into())
        })?
        .trim_end_matches('/');
    let identity = config.registered_router_identity().ok_or_else(|| {
        InstallationHeartbeatError::Transient("router installation is not registered".into())
    })?;
    let request = build_installation_heartbeat_request(identity, boot_id, public_ip)
        .map_err(|error| InstallationHeartbeatError::Transient(error.to_string()))?;
    let response = http
        .post(format!("{api_base}/v1/installations/heartbeat"))
        .json(&request)
        .timeout(ROUTER_INSTALLATION_HEARTBEAT_TIMEOUT)
        .send()
        .await
        .map_err(|error| InstallationHeartbeatError::Transient(error.to_string()))?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = read_bounded_router_error_body(response)
        .await
        .map_err(|error| InstallationHeartbeatError::Transient(error.to_string()))?;
    Err(classify_installation_heartbeat_failure(status, &body))
}

pub async fn send_installation_setup_completed(
    http: &reqwest::Client,
    config: &ServerConfig,
    setup: InstallationSetupCompletedPayload,
) -> Result<InstallationSetupCompletedAckStatus, InstallationSetupCompletedError> {
    let api_base = config
        .router_api_base()
        .ok_or_else(|| {
            InstallationSetupCompletedError::InvalidResponse(
                "router api base is not configured".to_string(),
            )
        })?
        .trim_end_matches('/');
    let identity = config.registered_router_identity().ok_or_else(|| {
        InstallationSetupCompletedError::InvalidResponse(
            "router installation is not registered".to_string(),
        )
    })?;
    let requested_setup_id = setup.setup_id.clone();
    let request = build_installation_setup_completed_request(identity, setup)
        .map_err(|error| InstallationSetupCompletedError::InvalidResponse(error.to_string()))?;
    let response = http
        .post(format!("{api_base}/v1/installations/setup-completed"))
        .json(&request)
        .timeout(ROUTER_SETUP_COMPLETED_TIMEOUT)
        .send()
        .await
        .map_err(InstallationSetupCompletedError::Request)?;
    let status = response.status();
    if !status.is_success() {
        let retry_after_ms = parse_retry_after_ms(response.headers());
        let body = read_bounded_router_error_body(response)
            .await
            .map_err(InstallationSetupCompletedError::Request)?;
        return Err(InstallationSetupCompletedError::Rejected {
            status,
            body,
            retry_after_ms,
        });
    }
    let response = response
        .json::<InstallationSetupCompletedResponse>()
        .await
        .map_err(|error| InstallationSetupCompletedError::InvalidResponse(error.to_string()))?;
    if !response.ok {
        return Err(InstallationSetupCompletedError::InvalidResponse(
            "router returned ok=false".to_string(),
        ));
    }
    validate_setup_id(&response.setup_id).map_err(|error| {
        InstallationSetupCompletedError::InvalidResponse(format!(
            "router acknowledgement setup id is invalid: {error}"
        ))
    })?;
    match response.status.as_str() {
        "queued" if response.setup_id == requested_setup_id => {
            Ok(InstallationSetupCompletedAckStatus::Queued)
        }
        "already_recorded" => {
            if response.setup_id != requested_setup_id {
                tracing::info!(
                    requested_setup_id = %requested_setup_id,
                    recorded_setup_id = %response.setup_id,
                    "router already recorded installation setup completion under another setup id"
                );
            }
            Ok(InstallationSetupCompletedAckStatus::AlreadyRecorded)
        }
        "suppressed_disabled" => {
            if response.setup_id != requested_setup_id {
                tracing::info!(
                    requested_setup_id = %requested_setup_id,
                    recorded_setup_id = %response.setup_id,
                    "router suppressed an existing installation setup completion under another setup id"
                );
            }
            Ok(InstallationSetupCompletedAckStatus::SuppressedDisabled)
        }
        "queued" => Err(InstallationSetupCompletedError::InvalidResponse(format!(
            "router acknowledgement setup id '{}' does not match request",
            response.setup_id
        ))),
        status => Err(InstallationSetupCompletedError::InvalidResponse(format!(
            "unsupported acknowledgement status '{status}'"
        ))),
    }
}

fn build_installation_setup_completed_request(
    identity: &RouterIdentity,
    setup: InstallationSetupCompletedPayload,
) -> anyhow::Result<SignedRequest<InstallationSetupCompletedEnvelope>> {
    let timestamp_ms = now_ms();
    let nonce = nonce();
    build_installation_setup_completed_request_at(identity, setup, timestamp_ms, nonce)
}

fn build_installation_setup_completed_request_at(
    identity: &RouterIdentity,
    setup: InstallationSetupCompletedPayload,
    timestamp_ms: i64,
    nonce: String,
) -> anyhow::Result<SignedRequest<InstallationSetupCompletedEnvelope>> {
    let signature = sign_payload(
        identity,
        INSTALLATION_SETUP_COMPLETED_ACTION,
        &setup,
        timestamp_ms,
        &nonce,
    )?;
    Ok(SignedRequest {
        protocol_epoch: PROTOCOL_EPOCH.to_string(),
        installation_id: identity.installation_id.clone(),
        timestamp_ms,
        nonce,
        signature,
        payload: InstallationSetupCompletedEnvelope { setup },
    })
}

fn validate_password_hint(password_hint: &str) -> anyhow::Result<()> {
    let bytes = password_hint.as_bytes();
    if bytes.len() != 8 || !password_hint.is_ascii() {
        bail!("password hint must contain exactly 8 ASCII characters");
    }
    if !bytes[1..7].iter().all(|byte| *byte == b'*') {
        bail!("password hint middle characters must be masked");
    }
    for byte in [bytes[0], bytes[7]] {
        if byte != b'*' && !byte.is_ascii_alphanumeric() {
            bail!("password hint visible characters must be ASCII alphanumeric");
        }
    }
    Ok(())
}

fn validate_setup_id(setup_id: &str) -> anyhow::Result<()> {
    let bytes = setup_id.as_bytes();
    let hyphens = [8, 13, 18, 23];
    if bytes.len() != 36
        || !setup_id.is_ascii()
        || bytes.iter().enumerate().any(|(index, byte)| {
            if hyphens.contains(&index) {
                *byte != b'-'
            } else {
                !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase()
            }
        })
        || bytes[14] != b'4'
        || !matches!(bytes[19], b'8' | b'9' | b'a' | b'b')
    {
        bail!("setup id must be a lowercase hyphenated UUID v4");
    }
    Ok(())
}

fn parse_retry_after_ms(headers: &reqwest::header::HeaderMap) -> Option<i64> {
    const MIN_RETRY_AFTER_SECS: u64 = 1;
    const MAX_RETRY_AFTER_SECS: u64 = 24 * 60 * 60;
    let seconds = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?
        .clamp(MIN_RETRY_AFTER_SECS, MAX_RETRY_AFTER_SECS);
    Some((seconds as i64).saturating_mul(1_000))
}

fn classify_installation_heartbeat_failure(
    status: reqwest::StatusCode,
    body: &str,
) -> InstallationHeartbeatError {
    let body = bounded_router_error_body(body);
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return InstallationHeartbeatError::RegistrationRequired { status, body };
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return InstallationHeartbeatError::EndpointUnavailable { status, body };
    }
    if status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
    {
        return InstallationHeartbeatError::Transient(format!("{status}: {body}"));
    }
    InstallationHeartbeatError::Rejected { status, body }
}

fn bounded_router_error_body(body: &str) -> String {
    body.chars().take(ROUTER_ERROR_BODY_LIMIT).collect()
}

fn build_installation_heartbeat_request(
    identity: &RouterIdentity,
    boot_id: &str,
    public_ip: Option<&str>,
) -> anyhow::Result<SignedRequest<InstallationHeartbeatPayload>> {
    let build = crate::build_info::build_info();
    signed_request(
        identity,
        "installation_heartbeat_v1",
        InstallationHeartbeatPayload {
            protocol_version: INSTALLATION_HEARTBEAT_PROTOCOL_VERSION,
            boot_id: boot_id.to_string(),
            app_version: crate::build_info::router_registration_version().to_string(),
            commit_id: build.commit_id.to_string(),
            public_ip: public_ip
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
        },
    )
}

pub async fn claim_client_tunnel(
    http: &reqwest::Client,
    config: &ServerConfig,
    tunnel: ClientTunnelConfig,
) -> anyhow::Result<()> {
    claim_client_tunnel_with_timeout(http, config, tunnel, ROUTER_CONTROL_PLANE_SYNC_TIMEOUT).await
}

async fn claim_client_tunnel_with_timeout(
    http: &reqwest::Client,
    config: &ServerConfig,
    tunnel: ClientTunnelConfig,
    timeout: Duration,
) -> anyhow::Result<()> {
    let api_base = config
        .router_api_base()
        .ok_or_else(|| anyhow::anyhow!("router api base is not configured"))?
        .trim_end_matches('/')
        .to_string();
    let identity = config
        .registered_router_identity()
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
        protocol_epoch: PROTOCOL_EPOCH,
        installation_id: identity.installation_id.clone(),
        timestamp_ms,
        nonce,
        signature,
        tunnel,
    };
    tokio::time::timeout(timeout, async {
        let response = http
            .post(format!("{api_base}/v1/installations/client-tunnel/claim"))
            .json(&request)
            .send()
            .await
            .map_err(ClientTunnelClaimError::Request)?;
        if response.status().is_success() {
            return Ok(());
        }
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(ClientTunnelClaimError::Request)?;
        Err(ClientTunnelClaimError::Rejected {
            status,
            body: bounded_router_error_body(&body),
        })
    })
    .await
    .map_err(|_| ClientTunnelClaimError::Timeout {
        timeout_seconds: timeout.as_secs_f64(),
    })?
    .map_err(anyhow::Error::new)
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
    check_client_tunnel_subdomain_available_with_timeout(
        http,
        router_api_base,
        subdomain,
        installation_id,
        ROUTER_CONTROL_PLANE_SYNC_TIMEOUT,
    )
    .await
}

async fn check_client_tunnel_subdomain_available_with_timeout(
    http: &reqwest::Client,
    router_api_base: &str,
    subdomain: &str,
    installation_id: Option<&str>,
    timeout: Duration,
) -> anyhow::Result<SubdomainAvailability> {
    tokio::time::timeout(
        timeout,
        check_client_tunnel_subdomain_available_request(
            http,
            router_api_base,
            subdomain,
            installation_id,
        ),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "router subdomain availability check timed out after {}s",
            timeout.as_secs_f64()
        )
    })?
}

async fn check_client_tunnel_subdomain_available_request(
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
        .registered_router_identity()
        .ok_or_else(|| anyhow::anyhow!("router installation is not registered"))?;
    let timestamp_ms = now_ms();
    let nonce = nonce();
    let empty = serde_json::json!({});
    let signature = sign_payload(identity, "client_tunnel_get", &empty, timestamp_ms, &nonce)?;
    let timestamp_ms = timestamp_ms.to_string();
    let response = http
        .get(format!("{api_base}/v1/installations/client-tunnel"))
        .query(&[
            ("protocolEpoch", PROTOCOL_EPOCH),
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

pub async fn get_installation_owner_email_status(
    http: &reqwest::Client,
    config: &ServerConfig,
) -> anyhow::Result<InstallationOwnerEmailStatus> {
    let api_base = config
        .router_api_base()
        .ok_or_else(|| anyhow::anyhow!("router api base is not configured"))?
        .trim_end_matches('/')
        .to_string();
    let identity = config
        .registered_router_identity()
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
            ("protocolEpoch", PROTOCOL_EPOCH),
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
        return Ok(body.into());
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
        .registered_router_identity()
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
        protocol_epoch: PROTOCOL_EPOCH,
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
    push_payout_profile_with_timeout(http, config, update, ROUTER_CONTROL_PLANE_SYNC_TIMEOUT).await
}

async fn push_payout_profile_with_timeout(
    http: &reqwest::Client,
    config: &ServerConfig,
    update: PayoutProfileState,
    timeout: Duration,
) -> anyhow::Result<InstallationPayoutProfileUpdateResponse> {
    tokio::time::timeout(timeout, push_payout_profile_request(http, config, update))
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "router payout profile sync timed out after {}s",
                timeout.as_secs_f64()
            )
        })?
}

async fn push_payout_profile_request(
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
        .registered_router_identity()
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
        protocol_epoch: PROTOCOL_EPOCH,
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

pub async fn issue_namespace_lease(
    http: &reqwest::Client,
    config: &ServerConfig,
    payload: NamespaceLeasePayload,
) -> anyhow::Result<NamespaceLeaseResponse> {
    validate_namespace_payload_header(config, &payload.protocol_epoch, &payload.router_id)?;
    let api_base = config
        .router_api_base()
        .ok_or_else(|| anyhow::anyhow!("router api base is not configured"))?
        .trim_end_matches('/');
    let identity = config
        .registered_router_identity()
        .ok_or_else(|| anyhow::anyhow!("router installation is not registered"))?;
    let request = tunnel_signed_request(identity, "tunnel_lease_issue", payload.clone())?;
    let response = http
        .post(format!("{api_base}/v1/tunnels/lease"))
        .json(&request)
        .send()
        .await
        .context("send router namespace tunnel lease")?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("router namespace tunnel lease failed: {status}: {body}");
    }
    let mut lease = response
        .json::<NamespaceLeaseResponse>()
        .await
        .context("parse router namespace tunnel lease response")?;
    validate_namespace_lease_response(&payload, &lease)?;
    normalize_namespace_lease_url_scheme(config, &mut lease);
    Ok(lease)
}

pub async fn renew_namespace_lease(
    http: &reqwest::Client,
    config: &ServerConfig,
    payload: NamespaceRenewLeasePayload,
) -> Result<NamespaceRenewLeaseResponse, RenewLeaseError> {
    validate_namespace_payload_header(config, &payload.protocol_epoch, &payload.router_id)
        .map_err(|error| RenewLeaseError::Terminal(error.to_string()))?;
    let api_base = config
        .router_api_base()
        .ok_or_else(|| RenewLeaseError::Terminal("router api base is not configured".into()))?
        .trim_end_matches('/');
    let identity = config
        .registered_router_identity()
        .ok_or_else(|| RenewLeaseError::Terminal("router installation is not registered".into()))?;
    let request = tunnel_signed_request(identity, "tunnel_lease_renew", payload.clone())
        .map_err(|error| RenewLeaseError::Terminal(error.to_string()))?;
    let response = http
        .post(format!("{api_base}/v1/tunnels/lease/renew"))
        .timeout(ROUTER_LEASE_RENEW_TIMEOUT)
        .json(&request)
        .send()
        .await
        .map_err(|error| {
            RenewLeaseError::Retryable(format!("send router namespace lease renewal: {error}"))
        })?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let message = format!("router namespace lease renewal failed: {status}: {body}");
        return if renew_status_is_retryable(status) {
            Err(RenewLeaseError::Retryable(message))
        } else {
            Err(RenewLeaseError::Terminal(message))
        };
    }
    let renewed = response
        .json::<NamespaceRenewLeaseResponse>()
        .await
        .map_err(|error| {
            RenewLeaseError::Retryable(format!(
                "parse router namespace lease renewal response: {error}"
            ))
        })?;
    if renewed.protocol_epoch != payload.protocol_epoch
        || renewed.router_id != payload.router_id
        || renewed.route_id != payload.route_id
        || renewed.rotation_id != payload.rotation_id
        || renewed.generation != payload.generation
    {
        return Err(RenewLeaseError::Terminal(
            "router namespace lease renewal identity mismatch".into(),
        ));
    }
    Ok(renewed)
}

pub async fn activate_namespace_tunnel(
    http: &reqwest::Client,
    config: &ServerConfig,
    payload: ActivateTunnelPayload,
) -> anyhow::Result<TunnelStateResponse> {
    send_namespace_tunnel_control(
        http,
        config,
        "/v1/tunnels/activate",
        "tunnel_activate",
        payload,
    )
    .await
}

pub async fn namespace_tunnel_state(
    http: &reqwest::Client,
    config: &ServerConfig,
    payload: TunnelStatePayload,
) -> anyhow::Result<TunnelStateResponse> {
    send_namespace_tunnel_control(http, config, "/v1/tunnels/state", "tunnel_state", payload).await
}

async fn send_namespace_tunnel_control<T: Serialize + Clone>(
    http: &reqwest::Client,
    config: &ServerConfig,
    path: &str,
    action: &str,
    payload: T,
) -> anyhow::Result<TunnelStateResponse> {
    let payload_value = serde_json::to_value(&payload).context("serialize tunnel control")?;
    let protocol_epoch = payload_value
        .get("protocolEpoch")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("tunnel control protocolEpoch is missing"))?;
    let router_id = payload_value
        .get("routerId")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("tunnel control routerId is missing"))?;
    validate_namespace_payload_header(config, protocol_epoch, router_id)?;
    let api_base = config
        .router_api_base()
        .ok_or_else(|| anyhow::anyhow!("router api base is not configured"))?
        .trim_end_matches('/');
    let identity = config
        .registered_router_identity()
        .ok_or_else(|| anyhow::anyhow!("router installation is not registered"))?;
    let request = tunnel_signed_request(identity, action, payload)?;
    let response = http
        .post(format!("{api_base}{path}"))
        .timeout(ROUTER_TUNNEL_CONTROL_HTTP_TIMEOUT)
        .json(&request)
        .send()
        .await
        .with_context(|| format!("send router tunnel control {action}"))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("router tunnel control {action} failed: {status}: {body}");
    }
    let state = response
        .json::<TunnelStateResponse>()
        .await
        .with_context(|| format!("parse router tunnel control {action} response"))?;
    if state.protocol_epoch != PROTOCOL_EPOCH || state.router_id != tunnel_router_id(config)? {
        bail!("router tunnel control {action} response identity mismatch");
    }
    Ok(state)
}

fn validate_namespace_payload_header(
    config: &ServerConfig,
    protocol_epoch: &str,
    router_id: &str,
) -> anyhow::Result<()> {
    if protocol_epoch != PROTOCOL_EPOCH {
        bail!("unsupported tunnel protocol epoch");
    }
    if router_id != tunnel_router_id(config)? {
        bail!("tunnel routerId does not match configured Router");
    }
    Ok(())
}

fn validate_namespace_lease_response(
    request: &NamespaceLeasePayload,
    response: &NamespaceLeaseResponse,
) -> anyhow::Result<()> {
    if response.protocol_epoch != request.protocol_epoch
        || response.router_id != request.router_id
        || response.route_id != request.route_id
        || response.rotation_id != request.rotation_id
        || response.generation != request.generation
        || response.expected_generation != request.expected_generation
        || response.subdomain != request.requested_subdomain
    {
        bail!("router namespace tunnel lease response identity mismatch");
    }
    Ok(())
}

fn normalize_namespace_lease_url_scheme(config: &ServerConfig, lease: &mut NamespaceLeaseResponse) {
    let router_url = config
        .router
        .url
        .as_deref()
        .or(config.router.api_base.as_deref())
        .unwrap_or_default();
    if router_url.starts_with("https://") && lease.tunnel_url.starts_with("http://") {
        lease.tunnel_url = format!("https://{}", lease.tunnel_url.trim_start_matches("http://"));
    }
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
    let identity = config
        .registered_router_identity()
        .ok_or_else(|| RenewLeaseError::Terminal("router installation is not registered".into()))?;
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
    let share = canonicalize_share_descriptor(config, share)?;
    let api_base = config
        .router_api_base()
        .ok_or_else(|| anyhow::anyhow!("router api base is not configured"))?
        .trim_end_matches('/')
        .to_string();
    let identity = config
        .registered_router_identity()
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
        protocol_epoch: PROTOCOL_EPOCH,
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
    let ops = canonicalize_share_operations(config, ops)?;
    push_share_ops_with_timeout(http, config, ops, ROUTER_CONTROL_PLANE_SYNC_TIMEOUT).await
}

pub async fn push_share_descriptor_ops(
    http: &reqwest::Client,
    config: &ServerConfig,
    ops: Vec<ShareSyncOperation>,
    strict_required: bool,
) -> anyhow::Result<ShareDescriptorSyncOutcome> {
    let ops = canonicalize_share_operations(config, ops)?;
    tokio::time::timeout(
        ROUTER_CONTROL_PLANE_SYNC_TIMEOUT,
        push_share_descriptor_ops_request(http, config, ops, strict_required),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "router Share descriptor sync timed out after {}s",
            ROUTER_CONTROL_PLANE_SYNC_TIMEOUT.as_secs_f64()
        )
    })?
}

fn canonicalize_share_operations(
    config: &ServerConfig,
    mut ops: Vec<ShareSyncOperation>,
) -> anyhow::Result<Vec<ShareSyncOperation>> {
    for operation in &mut ops {
        if let Some(share) = operation.share.take() {
            operation.share = Some(canonicalize_share_descriptor(config, share)?);
        }
    }
    Ok(ops)
}

pub fn canonicalize_share_descriptor(
    config: &ServerConfig,
    mut share: ShareDescriptor,
) -> anyhow::Result<ShareDescriptor> {
    let client_subdomain = config
        .client
        .tunnel_subdomain
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("client tunnel subdomain is not configured"))
        .and_then(|value| ClientSubdomain::parse(value).map_err(Into::into))?;
    let slug = if let Some((slug, suffix)) = share.subdomain.split_once("--") {
        let suffix = ClientSubdomain::parse(suffix)?;
        if suffix != client_subdomain {
            bail!("share host belongs to another client subdomain");
        }
        ShareSlug::parse(slug)?
    } else {
        ShareSlug::parse(&share.subdomain)?
    };
    share.subdomain = format!("{}--{}", slug.as_str(), client_subdomain.as_str());
    Ok(share)
}

pub async fn prune_shares(
    http: &reqwest::Client,
    config: &ServerConfig,
    share_ids: Vec<String>,
) -> anyhow::Result<SharePruneOutcome> {
    prune_shares_with_timeout(http, config, share_ids, ROUTER_CONTROL_PLANE_SYNC_TIMEOUT).await
}

async fn prune_shares_with_timeout(
    http: &reqwest::Client,
    config: &ServerConfig,
    share_ids: Vec<String>,
    timeout: Duration,
) -> anyhow::Result<SharePruneOutcome> {
    tokio::time::timeout(timeout, prune_shares_request(http, config, share_ids))
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "router share prune timed out after {}s",
                timeout.as_secs_f64()
            )
        })?
}

async fn prune_shares_request(
    http: &reqwest::Client,
    config: &ServerConfig,
    share_ids: Vec<String>,
) -> anyhow::Result<SharePruneOutcome> {
    if share_ids.len() > ROUTER_SHARE_PRUNE_MAX_IDS {
        bail!(
            "router share prune exceeds the {ROUTER_SHARE_PRUNE_MAX_IDS} share id limit: {}",
            share_ids.len()
        );
    }
    let api_base = config
        .router_api_base()
        .ok_or_else(|| anyhow::anyhow!("router api base is not configured"))?
        .trim_end_matches('/')
        .to_string();
    let identity = config
        .registered_router_identity()
        .ok_or_else(|| anyhow::anyhow!("router installation is not registered"))?;
    let request = build_share_prune_request(identity, share_ids, now_ms(), nonce())?;
    let response = http
        .post(format!("{api_base}/v1/shares/prune"))
        .json(&request)
        .send()
        .await
        .context("send router share prune")?;
    let status = response.status();
    if status.is_success() {
        return Ok(SharePruneOutcome::Applied);
    }
    if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::METHOD_NOT_ALLOWED
    {
        return Ok(SharePruneOutcome::Unsupported);
    }
    let body = read_bounded_router_error_body(response)
        .await
        .context("read router share prune error response")?;
    bail!("router share prune failed: {status}: {body}")
}

fn build_share_prune_request(
    identity: &RouterIdentity,
    share_ids: Vec<String>,
    timestamp_ms: i64,
    nonce: String,
) -> anyhow::Result<SharePruneRequest> {
    let signature = sign_payload(identity, "share_prune_v1", &share_ids, timestamp_ms, &nonce)?;
    Ok(SharePruneRequest {
        protocol_epoch: PROTOCOL_EPOCH,
        installation_id: identity.installation_id.clone(),
        timestamp_ms,
        nonce,
        signature,
        share_ids,
    })
}

async fn read_bounded_router_error_body(
    mut response: reqwest::Response,
) -> Result<String, reqwest::Error> {
    let mut body = Vec::with_capacity(ROUTER_ERROR_BODY_LIMIT);
    while body.len() < ROUTER_ERROR_BODY_LIMIT {
        let Some(chunk) = response.chunk().await? else {
            break;
        };
        if chunk.is_empty() {
            continue;
        }
        let remaining = ROUTER_ERROR_BODY_LIMIT - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    Ok(bounded_router_error_body(&String::from_utf8_lossy(&body)))
}

struct BoundedRouterResponseBody {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn read_bounded_router_registration_response_body(
    mut response: reqwest::Response,
) -> Result<BoundedRouterResponseBody, reqwest::Error> {
    let mut body = Vec::with_capacity(ROUTER_INSTALLATION_REGISTER_RESPONSE_BODY_LIMIT);
    loop {
        let Some(chunk) = response.chunk().await? else {
            break;
        };
        if chunk.is_empty() {
            continue;
        }
        let remaining = ROUTER_INSTALLATION_REGISTER_RESPONSE_BODY_LIMIT - body.len();
        if chunk.len() > remaining {
            body.extend_from_slice(&chunk[..remaining]);
            return Ok(BoundedRouterResponseBody {
                bytes: body,
                truncated: true,
            });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(BoundedRouterResponseBody {
        bytes: body,
        truncated: false,
    })
}

async fn push_share_ops_with_timeout(
    http: &reqwest::Client,
    config: &ServerConfig,
    ops: Vec<ShareSyncOperation>,
    timeout: Duration,
) -> anyhow::Result<()> {
    tokio::time::timeout(timeout, push_share_ops_request(http, config, ops))
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "router share batch sync timed out after {}s",
                timeout.as_secs_f64()
            )
        })?
}

async fn push_share_ops_request(
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
        .registered_router_identity()
        .ok_or_else(|| anyhow::anyhow!("router installation is not registered"))?;
    let response = send_share_ops_request(http, &api_base, identity, ops).await?;
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    bail!("router share batch sync failed: {status}: {body}");
}

async fn push_share_descriptor_ops_request(
    http: &reqwest::Client,
    config: &ServerConfig,
    ops: Vec<ShareSyncOperation>,
    strict_required: bool,
) -> anyhow::Result<ShareDescriptorSyncOutcome> {
    let api_base = config
        .router_api_base()
        .ok_or_else(|| anyhow::anyhow!("router api base is not configured"))?
        .trim_end_matches('/')
        .to_string();
    let identity = config
        .registered_router_identity()
        .ok_or_else(|| anyhow::anyhow!("router installation is not registered"))?;
    let response =
        send_share_descriptor_ops_request(http, &api_base, identity, ops.clone()).await?;
    if response.status().is_success() {
        let payload = response
            .json::<ShareDescriptorBatchSyncResponse>()
            .await
            .context("parse Router strict Share descriptor sync response")?;
        if !payload.ok {
            bail!("Router strict Share descriptor sync returned ok=false");
        }
        return Ok(ShareDescriptorSyncOutcome::Strict(payload.acks));
    }

    let status = response.status();
    if !strict_required
        && matches!(
            status,
            reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::METHOD_NOT_ALLOWED
        )
    {
        let response =
            send_share_ops_request(http, &api_base, identity, legacy_share_operations(ops)).await?;
        if response.status().is_success() {
            return Ok(ShareDescriptorSyncOutcome::Legacy);
        }
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("router legacy Share batch sync failed: {status}: {body}");
    }

    let body = response.text().await.unwrap_or_default();
    bail!("router strict Share descriptor sync failed: {status}: {body}");
}

fn legacy_share_operations(mut ops: Vec<ShareSyncOperation>) -> Vec<ShareSyncOperation> {
    for operation in &mut ops {
        if let Some(share) = operation.share.as_mut() {
            // Legacy Routers deserialize and reserialize the signed payload before
            // verification. Omit strict-only fields they do not know so both sides
            // build the same canonical JSON.
            share.descriptor_generation = 0;
            share.descriptor_fingerprint.clear();
        }
    }
    ops
}

async fn send_share_descriptor_ops_request(
    http: &reqwest::Client,
    api_base: &str,
    identity: &RouterIdentity,
    ops: Vec<ShareSyncOperation>,
) -> anyhow::Result<reqwest::Response> {
    let timestamp_ms = now_ms();
    let nonce = nonce();
    let signature = sign_payload(
        identity,
        "share_descriptor_batch_sync",
        &ops,
        timestamp_ms,
        &nonce,
    )?;
    let request = ShareBatchSyncRequest {
        protocol_epoch: PROTOCOL_EPOCH,
        installation_id: identity.installation_id.clone(),
        timestamp_ms,
        nonce,
        signature,
        ops,
    };
    http.post(format!("{api_base}/v1/shares/descriptor-batch-sync"))
        .json(&request)
        .send()
        .await
        .context("send Router strict Share descriptor sync")
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
        protocol_epoch: PROTOCOL_EPOCH,
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

pub async fn notify_runtime_refresh(
    http: &reqwest::Client,
    config: &ServerConfig,
    share_id: String,
    subdomain: String,
) -> anyhow::Result<()> {
    let client_subdomain = config
        .client
        .tunnel_subdomain
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("client tunnel subdomain is not configured"))
        .and_then(|value| ClientSubdomain::parse(value).map_err(Into::into))?;
    let subdomain = if let Some((slug, suffix)) = subdomain.split_once("--") {
        if ClientSubdomain::parse(suffix)? != client_subdomain {
            bail!("share host belongs to another client subdomain");
        }
        format!("{}--{}", ShareSlug::parse(slug)?, client_subdomain)
    } else {
        format!("{}--{}", ShareSlug::parse(&subdomain)?, client_subdomain)
    };
    let api_base = config
        .router_api_base()
        .ok_or_else(|| anyhow::anyhow!("router api base is not configured"))?
        .trim_end_matches('/')
        .to_string();
    let identity = config
        .registered_router_identity()
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
        protocol_epoch: PROTOCOL_EPOCH,
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
        .registered_router_identity()
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
        protocol_epoch: PROTOCOL_EPOCH,
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
        .registered_router_identity()
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
        .registered_router_identity()
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
        .registered_router_identity()
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
        "{api_base}/v1/shares/edit-events?protocolEpoch={PROTOCOL_EPOCH}&installationId={}&timestampMs={timestamp_ms}&nonce={}&signature={}",
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
        .registered_router_identity()
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
        protocol_epoch: PROTOCOL_EPOCH,
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
        protocol_epoch: PROTOCOL_EPOCH.to_string(),
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
        "{}\n{}\n{}\n{}\n{}\n{}",
        PROTOCOL_EPOCH, identity.installation_id, action, payload_json, timestamp_ms, nonce
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
        "{}\n{}\n{}\n{}\n{}\n{}",
        PROTOCOL_EPOCH,
        identity.installation_id,
        requested_subdomain,
        tunnel_type,
        timestamp_ms,
        nonce
    );
    Ok(STANDARD.encode(signing_key.sign(canonical.as_bytes()).to_bytes()))
}

pub(crate) fn generate_identity_without_installation() -> RouterIdentity {
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
    let signature = sign_registration_v2(
        identity,
        platform,
        app_version,
        &instance_nonce,
        timestamp_ms,
    )?;
    Ok(RegisterInstallationRequest {
        protocol_epoch: PROTOCOL_EPOCH.to_string(),
        public_key: identity.public_key.clone(),
        platform: platform.to_string(),
        app_version: app_version.to_string(),
        instance_nonce,
        proof_version: Some(REGISTRATION_PROOF_VERSION),
        timestamp_ms: Some(timestamp_ms),
        signature: Some(signature),
    })
}

fn build_legacy_register_installation_request(
    identity: &RouterIdentity,
    platform: &str,
    app_version: &str,
    instance_nonce: String,
    timestamp_ms: i64,
) -> anyhow::Result<RegisterInstallationRequest> {
    let signature = sign_registration_recovery(
        identity,
        platform,
        app_version,
        &instance_nonce,
        timestamp_ms,
    )?;
    Ok(RegisterInstallationRequest {
        protocol_epoch: PROTOCOL_EPOCH.to_string(),
        public_key: identity.public_key.clone(),
        platform: platform.to_string(),
        app_version: app_version.to_string(),
        instance_nonce,
        proof_version: None,
        timestamp_ms: Some(timestamp_ms),
        signature: Some(signature),
    })
}

fn build_unsigned_legacy_register_installation_request(
    identity: &RouterIdentity,
    platform: &str,
    app_version: &str,
    instance_nonce: String,
) -> RegisterInstallationRequest {
    RegisterInstallationRequest {
        protocol_epoch: PROTOCOL_EPOCH.to_string(),
        public_key: identity.public_key.clone(),
        platform: platform.to_string(),
        app_version: app_version.to_string(),
        instance_nonce,
        proof_version: None,
        timestamp_ms: None,
        signature: None,
    }
}

fn sign_registration_v2(
    identity: &RouterIdentity,
    platform: &str,
    app_version: &str,
    instance_nonce: &str,
    timestamp_ms: i64,
) -> anyhow::Result<String> {
    let secret = STANDARD
        .decode(&identity.private_key)
        .context("decode router private key")?;
    let secret: [u8; 32] = secret
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid router private key length"))?;
    let signing_key = SigningKey::from_bytes(&secret);
    let canonical = format!(
        "{}\nregister_installation_v2\n{}\n{}\n{}\n{}\n{}",
        PROTOCOL_EPOCH,
        identity.public_key.trim(),
        platform.trim(),
        app_version,
        instance_nonce,
        timestamp_ms
    );
    Ok(STANDARD.encode(signing_key.sign(canonical.as_bytes()).to_bytes()))
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
        "{}\n{}\nregister_installation\n{}\n{}\n{}\n{}\n{}",
        PROTOCOL_EPOCH,
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
        .registered_router_identity()
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
    use axum::body::Body;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::Response;
    use axum::routing::post;
    use axum::{Json, Router};
    use bytes::Bytes;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    use serde_json::{json, Value};
    use std::convert::Infallible;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct TestPayload {
        share_id: String,
    }

    fn oversized_chunked_response(status: StatusCode, prefix: &'static str) -> Response {
        let chunks = std::iter::once(Ok::<_, Infallible>(Bytes::from_static(prefix.as_bytes())))
            .chain(
                (0..=ROUTER_INSTALLATION_REGISTER_RESPONSE_BODY_LIMIT / 1024)
                    .map(|_| Ok::<_, Infallible>(Bytes::from_static(&[b'x'; 1024]))),
            );
        Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(Body::from_stream(futures_util::stream::iter(chunks)))
            .unwrap()
    }

    #[test]
    fn installation_owner_status_treats_legacy_visible_owner_as_verified() {
        let response: InstallationOwnerEmailResponse = serde_json::from_value(json!({
            "ok": true,
            "ownerEmail": "owner@example.com"
        }))
        .unwrap();
        let status = InstallationOwnerEmailStatus::from(response);

        assert_eq!(status.owner_email.as_deref(), Some("owner@example.com"));
        assert!(status.owner_verified);
    }

    #[test]
    fn installation_owner_status_preserves_explicit_unverified_state() {
        let response: InstallationOwnerEmailResponse = serde_json::from_value(json!({
            "ok": true,
            "ownerEmail": null,
            "ownerVerified": false
        }))
        .unwrap();
        let status = InstallationOwnerEmailStatus::from(response);

        assert_eq!(status.owner_email, None);
        assert!(!status.owner_verified);
    }

    #[test]
    fn share_descriptors_are_canonicalized_to_the_configured_client() {
        let mut config = ServerConfig::empty();
        config.client.tunnel_subdomain = Some("client-alpha".to_string());

        let descriptor = canonicalize_share_descriptor(
            &config,
            ShareDescriptor {
                subdomain: "codex-pro".to_string(),
                ..ShareDescriptor::default()
            },
        )
        .expect("raw Share slug must become a canonical Router label");
        assert_eq!(descriptor.subdomain, "codex-pro--client-alpha");

        let canonical = canonicalize_share_descriptor(
            &config,
            ShareDescriptor {
                subdomain: "codex-pro--client-alpha".to_string(),
                ..ShareDescriptor::default()
            },
        )
        .expect("canonical label must remain stable");
        assert_eq!(canonical.subdomain, "codex-pro--client-alpha");

        let error = canonicalize_share_descriptor(
            &config,
            ShareDescriptor {
                subdomain: "codex-pro--client-beta".to_string(),
                ..ShareDescriptor::default()
            },
        )
        .expect_err("another Client suffix must be rejected");
        assert!(error.to_string().contains("another client subdomain"));
    }

    #[test]
    fn share_batch_operations_canonicalize_upserts_only() {
        let mut config = ServerConfig::empty();
        config.client.tunnel_subdomain = Some("client-alpha".to_string());
        let operations = vec![
            ShareSyncOperation {
                kind: "upsert".to_string(),
                share: Some(ShareDescriptor {
                    subdomain: "claude-pro".to_string(),
                    ..ShareDescriptor::default()
                }),
                share_id: None,
            },
            ShareSyncOperation {
                kind: "delete".to_string(),
                share: None,
                share_id: Some("share-deleted".to_string()),
            },
        ];

        let operations = canonicalize_share_operations(&config, operations)
            .expect("batch canonicalization must succeed");
        assert_eq!(
            operations[0].share.as_ref().unwrap().subdomain,
            "claude-pro--client-alpha"
        );
        assert_eq!(operations[1].share_id.as_deref(), Some("share-deleted"));
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
        let canonical =
            "namespace-flat-1\ninst-1\nshare_delete\n{\"shareId\":\"share-1\"}\n123\nnonce-1";

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
    fn setup_completed_request_matches_cross_repository_signature_fixture() {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[7_u8; 32]);
        let identity = RouterIdentity {
            installation_id: "fixture-installation".to_string(),
            public_key: STANDARD.encode(signing_key.verifying_key().to_bytes()),
            private_key: STANDARD.encode(signing_key.to_bytes()),
            control_secret: None,
        };
        let setup = InstallationSetupCompletedPayload::new(
            "123e4567-e89b-42d3-a456-426614174000".to_string(),
            "p******w".to_string(),
        )
        .unwrap();

        let request = build_installation_setup_completed_request_at(
            &identity,
            setup,
            1_700_000_000_789,
            "fixture-setup-123".to_string(),
        )
        .unwrap();

        assert_eq!(
            request.signature,
            "q48WYcP91n3DWTvRyw9WysgC9AN5T3GM/2DyaDz18x2yKzyz/4iBkbXD+DYup6MtBtSGi+vEuaWhtO8kC4znCg=="
        );
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["protocolEpoch"], PROTOCOL_EPOCH);
        assert_eq!(value["installationId"], "fixture-installation");
        assert_eq!(value["timestampMs"], 1_700_000_000_789_i64);
        assert_eq!(value["nonce"], "fixture-setup-123");
        assert_eq!(value["setup"]["protocolVersion"], 1);
        assert_eq!(
            value["setup"]["setupId"],
            "123e4567-e89b-42d3-a456-426614174000"
        );
        assert_eq!(value["setup"]["passwordHint"], "p******w");
        assert!(value["setup"].get("passwordLength").is_none());
    }

    #[test]
    fn setup_completed_payload_rejects_noncanonical_ids_and_hints() {
        assert!(InstallationSetupCompletedPayload::new(
            "123E4567-E89B-42D3-A456-426614174000".to_string(),
            "p******w".to_string(),
        )
        .is_err());
        assert!(InstallationSetupCompletedPayload::new(
            "123e4567-e89b-42d3-a456-426614174000".to_string(),
            "p*****w".to_string(),
        )
        .is_err());
        assert!(InstallationSetupCompletedPayload::new(
            "123e4567-e89b-42d3-a456-426614174000".to_string(),
            "!******?".to_string(),
        )
        .is_err());
    }

    #[tokio::test]
    async fn setup_completed_acknowledgement_validates_status_setup_id_rules() {
        async fn already_recorded() -> Json<Value> {
            Json(json!({
                "ok": true,
                "status": "already_recorded",
                "setupId": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
            }))
        }

        let app = Router::new().route("/v1/installations/setup-completed", post(already_recorded));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut config = ServerConfig::empty();
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "fixture-installation".to_string();
        config.router.identity = Some(identity);
        let setup = InstallationSetupCompletedPayload::new(
            "123e4567-e89b-42d3-a456-426614174000".to_string(),
            "p******w".to_string(),
        )
        .unwrap();

        assert_eq!(
            send_installation_setup_completed(&reqwest::Client::new(), &config, setup)
                .await
                .unwrap(),
            InstallationSetupCompletedAckStatus::AlreadyRecorded
        );
        server.abort();

        async fn mismatched_queued() -> Json<Value> {
            Json(json!({
                "ok": true,
                "status": "queued",
                "setupId": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
            }))
        }
        let app = Router::new().route("/v1/installations/setup-completed", post(mismatched_queued));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        config.router.url = Some(format!("http://{addr}"));
        let setup = InstallationSetupCompletedPayload::new(
            "123e4567-e89b-42d3-a456-426614174000".to_string(),
            "p******w".to_string(),
        )
        .unwrap();

        assert!(matches!(
            send_installation_setup_completed(&reqwest::Client::new(), &config, setup).await,
            Err(InstallationSetupCompletedError::InvalidResponse(_))
        ));
        server.abort();

        async fn mismatched_suppressed() -> Json<Value> {
            Json(json!({
                "ok": true,
                "status": "suppressed_disabled",
                "setupId": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
            }))
        }
        let app = Router::new().route(
            "/v1/installations/setup-completed",
            post(mismatched_suppressed),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        config.router.url = Some(format!("http://{addr}"));
        let setup = InstallationSetupCompletedPayload::new(
            "123e4567-e89b-42d3-a456-426614174000".to_string(),
            "p******w".to_string(),
        )
        .unwrap();

        assert_eq!(
            send_installation_setup_completed(&reqwest::Client::new(), &config, setup)
                .await
                .unwrap(),
            InstallationSetupCompletedAckStatus::SuppressedDisabled
        );
        server.abort();

        async fn malformed_existing_id() -> Json<Value> {
            Json(json!({
                "ok": true,
                "status": "already_recorded",
                "setupId": "legacy-marker"
            }))
        }
        let app = Router::new().route(
            "/v1/installations/setup-completed",
            post(malformed_existing_id),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        config.router.url = Some(format!("http://{addr}"));
        let setup = InstallationSetupCompletedPayload::new(
            "123e4567-e89b-42d3-a456-426614174000".to_string(),
            "p******w".to_string(),
        )
        .unwrap();

        assert!(matches!(
            send_installation_setup_completed(&reqwest::Client::new(), &config, setup).await,
            Err(InstallationSetupCompletedError::InvalidResponse(message))
                if message.contains("setup id is invalid")
        ));
        server.abort();
    }

    #[test]
    fn setup_completed_error_classification_keeps_rollout_failures_retryable() {
        for status in [
            reqwest::StatusCode::NOT_FOUND,
            reqwest::StatusCode::METHOD_NOT_ALLOWED,
            reqwest::StatusCode::UNAUTHORIZED,
            reqwest::StatusCode::CONFLICT,
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
        ] {
            assert!(!InstallationSetupCompletedError::Rejected {
                status,
                body: "retry".to_string(),
                retry_after_ms: None,
            }
            .is_terminal());
        }
        for status in [
            reqwest::StatusCode::BAD_REQUEST,
            reqwest::StatusCode::FORBIDDEN,
            reqwest::StatusCode::UNPROCESSABLE_ENTITY,
        ] {
            assert!(InstallationSetupCompletedError::Rejected {
                status,
                body: "terminal".to_string(),
                retry_after_ms: None,
            }
            .is_terminal());
        }
    }

    #[test]
    fn setup_completed_retry_after_seconds_are_clamped() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "0".parse().unwrap());
        assert_eq!(parse_retry_after_ms(&headers), Some(1_000));

        headers.insert(reqwest::header::RETRY_AFTER, "3600".parse().unwrap());
        assert_eq!(parse_retry_after_ms(&headers), Some(3_600_000));

        headers.insert(reqwest::header::RETRY_AFTER, "999999".parse().unwrap());
        assert_eq!(parse_retry_after_ms(&headers), Some(24 * 60 * 60 * 1_000));

        headers.insert(
            reqwest::header::RETRY_AFTER,
            "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(parse_retry_after_ms(&headers), None);
    }

    #[test]
    fn share_prune_request_signs_exact_share_ids_payload() {
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-prune".to_string();
        let request = build_share_prune_request(
            &identity,
            vec!["share-b".to_string(), "share-a".to_string()],
            123,
            "prune-nonce".to_string(),
        )
        .unwrap();

        let public_key = STANDARD.decode(&identity.public_key).unwrap();
        let public_key: [u8; 32] = public_key.try_into().unwrap();
        let verifying_key = VerifyingKey::from_bytes(&public_key).unwrap();
        let signature = STANDARD.decode(&request.signature).unwrap();
        let signature: [u8; 64] = signature.try_into().unwrap();
        let signature = Signature::from_bytes(&signature);
        let canonical = "namespace-flat-1\ninst-prune\nshare_prune_v1\n[\"share-b\",\"share-a\"]\n123\nprune-nonce";

        verifying_key
            .verify(canonical.as_bytes(), &signature)
            .unwrap();
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["installationId"], "inst-prune");
        assert_eq!(value["shareIds"], json!(["share-b", "share-a"]));
        assert!(value.get("payload").is_none());
    }

    #[test]
    fn first_registration_request_carries_v2_proof_of_possession() {
        let identity = generate_identity_without_installation();

        let request = build_register_installation_request(
            &identity,
            "linux",
            "1.2.3",
            "registration-nonce".into(),
            123,
        )
        .unwrap();

        assert_eq!(request.proof_version, Some(2));
        assert_eq!(request.timestamp_ms, Some(123));
        let public_key = STANDARD.decode(&identity.public_key).unwrap();
        let public_key: [u8; 32] = public_key.try_into().unwrap();
        let verifying_key = VerifyingKey::from_bytes(&public_key).unwrap();
        let signature = STANDARD.decode(request.signature.unwrap()).unwrap();
        let signature: [u8; 64] = signature.try_into().unwrap();
        let signature = Signature::from_bytes(&signature);
        let canonical = format!(
            "{PROTOCOL_EPOCH}\nregister_installation_v2\n{}\nlinux\n1.2.3\nregistration-nonce\n123",
            identity.public_key
        );

        verifying_key
            .verify(canonical.as_bytes(), &signature)
            .unwrap();
    }

    #[test]
    fn existing_registration_request_still_uses_id_independent_v2_proof() {
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

        assert_eq!(request.proof_version, Some(2));
        let public_key = STANDARD.decode(&identity.public_key).unwrap();
        let public_key: [u8; 32] = public_key.try_into().unwrap();
        let verifying_key = VerifyingKey::from_bytes(&public_key).unwrap();
        let signature = STANDARD.decode(request.signature.unwrap()).unwrap();
        let signature: [u8; 64] = signature.try_into().unwrap();
        let signature = Signature::from_bytes(&signature);
        let canonical = format!(
            "{PROTOCOL_EPOCH}\nregister_installation_v2\n{}\nlinux\n1.2.3\nregistration-nonce\n123",
            identity.public_key
        );

        verifying_key
            .verify(canonical.as_bytes(), &signature)
            .unwrap();
    }

    #[test]
    fn registration_recovery_signature_is_epoch_scoped() {
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-existing".into();

        let request = build_legacy_register_installation_request(
            &identity,
            "linux",
            "1.2.3",
            "legacy-nonce".into(),
            456,
        )
        .unwrap();

        assert_eq!(request.proof_version, None);
        let public_key = STANDARD.decode(&identity.public_key).unwrap();
        let public_key: [u8; 32] = public_key.try_into().unwrap();
        let verifying_key = VerifyingKey::from_bytes(&public_key).unwrap();
        let signature = STANDARD.decode(request.signature.unwrap()).unwrap();
        let signature: [u8; 64] = signature.try_into().unwrap();
        let signature = Signature::from_bytes(&signature);
        let canonical = format!(
            "{PROTOCOL_EPOCH}\ninst-existing\nregister_installation\n{}\nlinux\n1.2.3\nlegacy-nonce\n456",
            identity.public_key
        );

        verifying_key
            .verify(canonical.as_bytes(), &signature)
            .unwrap();
    }

    #[test]
    fn heartbeat_request_is_flattened_and_matches_signed_request_canonical() {
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-heartbeat".into();

        let request = build_installation_heartbeat_request(&identity, "boot-123", None).unwrap();
        let payload_json = serde_json::to_string(&request.payload).unwrap();
        let canonical = format!(
            "{PROTOCOL_EPOCH}\ninst-heartbeat\ninstallation_heartbeat_v1\n{}\n{}\n{}",
            payload_json, request.timestamp_ms, request.nonce
        );
        let public_key = STANDARD.decode(&identity.public_key).unwrap();
        let public_key: [u8; 32] = public_key.try_into().unwrap();
        let verifying_key = VerifyingKey::from_bytes(&public_key).unwrap();
        let signature = STANDARD.decode(&request.signature).unwrap();
        let signature: [u8; 64] = signature.try_into().unwrap();
        let signature = Signature::from_bytes(&signature);

        verifying_key
            .verify(canonical.as_bytes(), &signature)
            .unwrap();
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["installationId"], "inst-heartbeat");
        assert_eq!(value["protocolVersion"], 1);
        assert_eq!(value["bootId"], "boot-123");
        assert_eq!(
            value["appVersion"],
            crate::build_info::router_registration_version()
        );
        assert_eq!(value["commitId"], env!("CC_SWITCH_BUILD_COMMIT"));
        assert!(value.get("payload").is_none());
    }

    #[test]
    fn cross_repo_registration_and_heartbeat_contract_vectors_are_stable() {
        let identity = RouterIdentity {
            installation_id: "fixture-installation".into(),
            public_key: "6kpsY+KcUgq+9VB7Ey7F+ZVHdq6+vnuSQh7qaRRG0iw=".into(),
            private_key: "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=".into(),
            control_secret: None,
        };
        let registration = build_register_installation_request(
            &identity,
            "linux",
            "1.2.3",
            "fixture-nonce-123".into(),
            1_700_000_000_123,
        )
        .unwrap();
        assert_eq!(registration.proof_version, Some(2));
        assert_eq!(
            registration.signature.as_deref(),
            Some(
                "nlRT3f2KJ0oZaI84N/naU1WYGv/bS7Pz0X7I0hDKxQg2U0RZ/eZhmpZ4yaCcTARWq7TRvaGbUe7vejXPnmkcBA=="
            )
        );

        let legacy = build_legacy_register_installation_request(
            &identity,
            "linux",
            "1.2.3",
            "fixture-legacy-123".into(),
            1_700_000_000_234,
        )
        .unwrap();
        assert_eq!(
            legacy.signature.as_deref(),
            Some(
                "+SCyz8ys5tyXjoTXYkzIyb9n/LBovygmFHz4wHoDF7uJF7jNKh7egVmaUGK9E34nyWytM1fPTyoMrl+TRh4tCg=="
            )
        );

        let payload = InstallationHeartbeatPayload {
            protocol_version: 1,
            boot_id: "fixture-boot".into(),
            app_version: "1.2.3".into(),
            commit_id: "abcdef123456".into(),
            public_ip: None,
        };
        let signature = sign_payload(
            &identity,
            "installation_heartbeat_v1",
            &payload,
            1_700_000_000_456,
            "fixture-heartbeat-123",
        )
        .unwrap();
        assert_eq!(
            signature,
            "Ax5dl8lsVWD1wh/8qxs72+hrPtRjjMdJzX/22gQDLxpwClbmIcUThVrGVQH5n1hVuwZsvKfYuoLCINMuGbg8Cg=="
        );
        let value = serde_json::to_value(SignedRequest {
            protocol_epoch: PROTOCOL_EPOCH.to_string(),
            installation_id: identity.installation_id,
            timestamp_ms: 1_700_000_000_456,
            nonce: "fixture-heartbeat-123".into(),
            signature,
            payload,
        })
        .unwrap();
        assert_eq!(value["protocolVersion"], 1);
        assert_eq!(value["bootId"], "fixture-boot");
        assert!(value.get("payload").is_none());
    }

    #[test]
    fn heartbeat_failure_classification_distinguishes_old_endpoint_and_missing_identity() {
        assert!(matches!(
            classify_installation_heartbeat_failure(
                reqwest::StatusCode::NOT_FOUND,
                "route not found"
            ),
            InstallationHeartbeatError::EndpointUnavailable { .. }
        ));
        assert!(matches!(
            classify_installation_heartbeat_failure(
                reqwest::StatusCode::NOT_FOUND,
                "installation not found"
            ),
            InstallationHeartbeatError::EndpointUnavailable { .. }
        ));
        assert!(matches!(
            classify_installation_heartbeat_failure(
                reqwest::StatusCode::NOT_FOUND,
                "route /v1/installations/heartbeat not found"
            ),
            InstallationHeartbeatError::EndpointUnavailable { .. }
        ));
        assert!(matches!(
            classify_installation_heartbeat_failure(
                reqwest::StatusCode::UNAUTHORIZED,
                "invalid signature"
            ),
            InstallationHeartbeatError::RegistrationRequired { .. }
        ));
        assert!(matches!(
            classify_installation_heartbeat_failure(
                reqwest::StatusCode::SERVICE_UNAVAILABLE,
                "retry later"
            ),
            InstallationHeartbeatError::Transient(_)
        ));
    }

    #[tokio::test]
    async fn registration_v2_rejection_does_not_enable_legacy_fallback() {
        async fn handler(
            State(requests): State<Arc<Mutex<Vec<Value>>>>,
            Json(request): Json<Value>,
        ) -> (StatusCode, Json<Value>) {
            let mut requests = requests.lock().await;
            requests.push(request);
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"message": "arbitrary old router rejection"})),
            )
        }

        let requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1/installations/register", post(handler))
            .with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-existing".into();

        let error = register_installation_v2(
            &reqwest::Client::new(),
            &format!("http://{addr}"),
            &identity,
        )
        .await
        .unwrap_err();

        assert!(!error.allows_legacy_fallback());
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["proofVersion"], 2);
        server.abort();
    }

    #[tokio::test]
    async fn registration_total_timeout_covers_a_stalled_response_body() {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 128\r\n\r\n{",
                )
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let identity = generate_identity_without_installation();
        let request = build_register_installation_request(
            &identity,
            "linux",
            "1.2.3",
            "timeout-nonce".into(),
            123,
        )
        .unwrap();

        let error = send_register_installation_request_with_timeout(
            &reqwest::Client::new(),
            &format!("http://{addr}"),
            &request,
            Duration::from_millis(25),
        )
        .await
        .expect_err("stalled response body must hit the total request timeout");

        let RegisterInstallationAttemptError::Request(error) = error else {
            panic!("stalled response must be classified as a request failure");
        };
        assert!(error.is_timeout());
        server.abort();
    }

    #[tokio::test]
    async fn registration_rejects_an_oversized_chunked_success_response() {
        async fn handler() -> Response {
            oversized_chunked_response(StatusCode::OK, "{\"installationId\":\"")
        }

        let app = Router::new().route("/v1/installations/register", post(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let identity = generate_identity_without_installation();
        let request = build_register_installation_request(
            &identity,
            "linux",
            "1.2.3",
            "oversized-response-nonce".into(),
            123,
        )
        .unwrap();

        let error = send_register_installation_request_with_timeout(
            &reqwest::Client::new(),
            &format!("http://{addr}"),
            &request,
            Duration::from_secs(1),
        )
        .await
        .expect_err("an oversized success response must be rejected");

        assert!(matches!(
            error,
            RegisterInstallationAttemptError::InvalidResponse(message)
                if message.contains("response body exceeds the 16384 byte limit")
        ));
        server.abort();
    }

    #[tokio::test]
    async fn registration_preserves_rejection_for_an_oversized_chunked_error() {
        async fn handler() -> Response {
            oversized_chunked_response(StatusCode::UNAUTHORIZED, "registration denied: ")
        }

        let app = Router::new().route("/v1/installations/register", post(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let identity = generate_identity_without_installation();

        let error = register_installation_v2(
            &reqwest::Client::new(),
            &format!("http://{addr}"),
            &identity,
        )
        .await
        .expect_err("an oversized rejection must remain a rejection");

        let RegisterInstallationAttemptError::Rejected { status, body } = error else {
            panic!("oversized error response changed registration error classification");
        };
        assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
        assert!(body.starts_with("registration denied: "));
        assert_eq!(body.chars().count(), ROUTER_ERROR_BODY_LIMIT);
        server.abort();
    }

    #[test]
    fn unsigned_legacy_discovery_omits_all_proof_fields() {
        let identity = generate_identity_without_installation();
        let request = build_unsigned_legacy_register_installation_request(
            &identity,
            "linux",
            "1.2.3",
            "discovery-nonce".into(),
        );
        let value = serde_json::to_value(request).unwrap();

        assert!(value.get("proofVersion").is_none());
        assert!(value.get("timestampMs").is_none());
        assert!(value.get("signature").is_none());
        assert_eq!(value["instanceNonce"], "discovery-nonce");
    }

    #[tokio::test]
    async fn heartbeat_http_failure_returns_without_blocking_the_caller() {
        async fn handler() -> (StatusCode, Json<Value>) {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"message": "temporarily unavailable"})),
            )
        }

        let app = Router::new().route("/v1/installations/heartbeat", post(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut config = ServerConfig::empty();
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-heartbeat".into();
        config.router.identity = Some(identity);

        let error = tokio::time::timeout(
            Duration::from_secs(1),
            send_installation_heartbeat(&reqwest::Client::new(), &config, "boot-123", None),
        )
        .await
        .expect("heartbeat failure should return promptly")
        .expect_err("a non-success response must be reported");

        assert!(error.to_string().contains("503 Service Unavailable"));
        server.abort();
    }

    #[tokio::test]
    async fn heartbeat_preserves_classification_for_an_oversized_chunked_error() {
        async fn handler() -> Response {
            oversized_chunked_response(StatusCode::SERVICE_UNAVAILABLE, "router overloaded: ")
        }

        let app = Router::new().route("/v1/installations/heartbeat", post(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut config = ServerConfig::empty();
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-heartbeat-oversized".into();
        config.router.identity = Some(identity);

        let error = send_installation_heartbeat(
            &reqwest::Client::new(),
            &config,
            "boot-oversized-response",
            None,
        )
        .await
        .expect_err("an oversized heartbeat error must be reported");

        let InstallationHeartbeatError::Transient(message) = error else {
            panic!("oversized error response changed heartbeat error classification");
        };
        assert!(message.contains("503 Service Unavailable: router overloaded: "));
        assert!(message.chars().count() < ROUTER_ERROR_BODY_LIMIT + 40);
        server.abort();
    }

    #[tokio::test]
    async fn heartbeat_classifies_401_and_404_without_waiting_for_a_stalled_chunked_tail() {
        use tokio::io::AsyncWriteExt;

        for (status, status_line) in [
            (reqwest::StatusCode::UNAUTHORIZED, "401 Unauthorized"),
            (reqwest::StatusCode::NOT_FOUND, "404 Not Found"),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let body = "x".repeat(ROUTER_ERROR_BODY_LIMIT);
                let headers = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n",
                    body.len()
                );
                socket.write_all(headers.as_bytes()).await.unwrap();
                socket.write_all(body.as_bytes()).await.unwrap();
                socket.write_all(b"\r\n").await.unwrap();
                tokio::time::sleep(Duration::from_secs(5)).await;
            });
            let mut config = ServerConfig::empty();
            config.router.url = Some(format!("http://{addr}"));
            let mut identity = generate_identity_without_installation();
            identity.installation_id = format!("inst-heartbeat-stalled-{}", status.as_u16());
            config.router.identity = Some(identity);

            let error = tokio::time::timeout(
                Duration::from_secs(1),
                send_installation_heartbeat(
                    &reqwest::Client::new(),
                    &config,
                    "boot-stalled-error-tail",
                    None,
                ),
            )
            .await
            .expect("heartbeat must stop reading after the bounded error prefix")
            .expect_err("the heartbeat response must remain an error");

            let body = match (status, error) {
                (
                    reqwest::StatusCode::UNAUTHORIZED,
                    InstallationHeartbeatError::RegistrationRequired { body, .. },
                ) => body,
                (
                    reqwest::StatusCode::NOT_FOUND,
                    InstallationHeartbeatError::EndpointUnavailable { body, .. },
                ) => body,
                (_, error) => panic!("stalled error tail changed classification: {error}"),
            };
            assert_eq!(body.len(), ROUTER_ERROR_BODY_LIMIT);
            server.abort();
        }
    }

    #[tokio::test]
    async fn share_sync_has_one_total_http_timeout() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let mut config = ServerConfig::empty();
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-share-sync-timeout".into();
        config.router.identity = Some(identity);

        let error = push_share_ops_with_timeout(
            &reqwest::Client::new(),
            &config,
            Vec::new(),
            Duration::from_millis(20),
        )
        .await
        .expect_err("a stalled share sync must hit the operation deadline");

        assert!(error.to_string().contains("share batch sync timed out"));
        server.abort();
    }

    #[tokio::test]
    async fn descriptor_sync_falls_back_only_before_strict_mode_is_sticky() {
        type RecordedRequests = Arc<Mutex<Vec<(String, Value)>>>;

        async fn strict_handler(
            State(requests): State<RecordedRequests>,
            Json(request): Json<Value>,
        ) -> StatusCode {
            requests.lock().await.push(("strict".into(), request));
            StatusCode::NOT_FOUND
        }
        async fn legacy_handler(
            State(requests): State<RecordedRequests>,
            Json(request): Json<Value>,
        ) -> Json<Value> {
            requests.lock().await.push(("legacy".into(), request));
            Json(json!({"ok": true}))
        }

        let requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1/shares/descriptor-batch-sync", post(strict_handler))
            .route("/v1/shares/batch-sync", post(legacy_handler))
            .with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut config = ServerConfig::empty();
        config.router.url = Some(format!("http://{addr}"));
        config.client.tunnel_subdomain = Some("client-alpha".into());
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-descriptor-fallback".into();
        config.router.identity = Some(identity);
        let ops = vec![ShareSyncOperation {
            kind: "upsert".into(),
            share_id: None,
            share: Some(ShareDescriptor {
                share_id: "share-descriptor-fallback".into(),
                share_name: "Descriptor fallback".into(),
                subdomain: "fallback".into(),
                descriptor_generation: 7,
                descriptor_fingerprint: "descriptor-fingerprint".into(),
                ..ShareDescriptor::default()
            }),
        }];

        assert_eq!(
            push_share_descriptor_ops(&reqwest::Client::new(), &config, ops.clone(), false)
                .await
                .unwrap(),
            ShareDescriptorSyncOutcome::Legacy
        );
        let captured = requests.lock().await.clone();
        assert_eq!(captured.len(), 2);
        let strict_share = &captured[0].1["ops"][0]["share"];
        assert_eq!(strict_share["descriptorGeneration"], 7);
        assert_eq!(
            strict_share["descriptorFingerprint"],
            "descriptor-fingerprint"
        );
        let legacy_request = &captured[1].1;
        let legacy_share = &legacy_request["ops"][0]["share"];
        assert!(legacy_share.get("descriptorGeneration").is_none());
        assert!(legacy_share.get("descriptorFingerprint").is_none());

        let expected_legacy_ops =
            legacy_share_operations(canonicalize_share_operations(&config, ops.clone()).unwrap());
        let expected_signature = sign_payload(
            config.registered_router_identity().unwrap(),
            "share_batch_sync",
            &expected_legacy_ops,
            legacy_request["timestampMs"].as_i64().unwrap(),
            legacy_request["nonce"].as_str().unwrap(),
        )
        .unwrap();
        assert_eq!(
            legacy_request["signature"].as_str(),
            Some(expected_signature.as_str())
        );

        let error = push_share_descriptor_ops(&reqwest::Client::new(), &config, ops, true)
            .await
            .expect_err("sticky strict mode must not downgrade after a 404");
        assert!(error.to_string().contains("404 Not Found"));
        server.abort();
    }

    #[tokio::test]
    async fn descriptor_sync_parses_strict_ack_envelope() {
        async fn handler() -> Json<Value> {
            Json(json!({"ok": true, "acks": []}))
        }
        let app = Router::new().route("/v1/shares/descriptor-batch-sync", post(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut config = ServerConfig::empty();
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-descriptor-strict".into();
        config.router.identity = Some(identity);

        assert_eq!(
            push_share_descriptor_ops(&reqwest::Client::new(), &config, Vec::new(), false)
                .await
                .unwrap(),
            ShareDescriptorSyncOutcome::Strict(Vec::new())
        );
        server.abort();
    }

    #[tokio::test]
    async fn share_prune_classifies_only_404_and_405_as_unsupported() {
        async fn handler(Json(request): Json<Value>) -> (StatusCode, String) {
            match request["shareIds"][0].as_str().unwrap_or_default() {
                "unsupported-404" => (StatusCode::NOT_FOUND, "missing".to_string()),
                "unsupported-405" => (StatusCode::METHOD_NOT_ALLOWED, "old".to_string()),
                "failure" => (StatusCode::CONFLICT, "x".repeat(4096)),
                _ => (StatusCode::NO_CONTENT, String::new()),
            }
        }

        let app = Router::new().route("/v1/shares/prune", post(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut config = ServerConfig::empty();
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-prune-status".into();
        config.router.identity = Some(identity);

        assert_eq!(
            prune_shares(
                &reqwest::Client::new(),
                &config,
                vec!["applied".to_string()]
            )
            .await
            .unwrap(),
            SharePruneOutcome::Applied
        );
        for share_id in ["unsupported-404", "unsupported-405"] {
            assert_eq!(
                prune_shares(&reqwest::Client::new(), &config, vec![share_id.to_string()])
                    .await
                    .unwrap(),
                SharePruneOutcome::Unsupported
            );
        }
        let error = prune_shares(
            &reqwest::Client::new(),
            &config,
            vec!["failure".to_string()],
        )
        .await
        .expect_err("non-404/405 failures must remain retryable errors");
        assert!(error.to_string().contains("409 Conflict"));
        assert!(error.to_string().len() < 600, "{error:#}");

        let error = prune_shares(
            &reqwest::Client::new(),
            &config,
            vec!["share".to_string(); ROUTER_SHARE_PRUNE_MAX_IDS + 1],
        )
        .await
        .expect_err("an oversized prune payload must be rejected before sending");
        assert!(error.to_string().contains("10000 share id limit"));
        server.abort();
    }

    #[tokio::test]
    async fn share_prune_response_loss_is_not_classified_as_applied() {
        use tokio::io::AsyncReadExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let received = socket.read(&mut request).await.unwrap();
            assert!(received > 0);
        });
        let mut config = ServerConfig::empty();
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-prune-lost-response".into();
        config.router.identity = Some(identity);

        prune_shares_with_timeout(
            &reqwest::Client::new(),
            &config,
            vec!["share-lost-response".to_string()],
            Duration::from_secs(1),
        )
        .await
        .expect_err("a lost response must not be treated as an applied prune");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn share_prune_timeout_covers_stalled_error_body() {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/plain\r\nContent-Length: 128\r\n\r\nx",
                )
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let mut config = ServerConfig::empty();
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-prune-timeout".into();
        config.router.identity = Some(identity);

        let error = prune_shares_with_timeout(
            &reqwest::Client::new(),
            &config,
            vec!["share-prune-timeout".to_string()],
            Duration::from_millis(20),
        )
        .await
        .expect_err("a stalled prune error body must hit the total deadline");

        assert!(error.to_string().contains("share prune timed out"));
        server.abort();
    }

    #[tokio::test]
    async fn payout_sync_has_one_total_http_timeout() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let mut config = ServerConfig::empty();
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-payout-sync-timeout".into();
        config.router.identity = Some(identity);
        let update = PayoutProfileState {
            schema_version: crate::domain::settings::config::PAYOUT_PROFILE_SCHEMA_VERSION,
            revision: 1,
            profile: None,
            updated_at_ms: 1,
        };

        let error = push_payout_profile_with_timeout(
            &reqwest::Client::new(),
            &config,
            update,
            Duration::from_millis(20),
        )
        .await
        .expect_err("a stalled payout sync must hit the operation deadline");

        assert!(error.to_string().contains("payout profile sync timed out"));
        server.abort();
    }

    #[tokio::test]
    async fn client_tunnel_claim_preserves_typed_http_status() {
        async fn handler(Json(request): Json<Value>) -> (StatusCode, Json<Value>) {
            if request["tunnel"]["subdomain"] == "permanent" {
                (
                    StatusCode::CONFLICT,
                    Json(json!({"message": "connection belongs to another owner"})),
                )
            } else {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"message": "retry later"})),
                )
            }
        }

        let app = Router::new().route("/v1/installations/client-tunnel/claim", post(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut config = ServerConfig::empty();
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-claim-status".into();
        config.router.identity = Some(identity);

        let permanent = claim_client_tunnel(
            &reqwest::Client::new(),
            &config,
            ClientTunnelConfig {
                owner_email: "owner@example.com".into(),
                subdomain: "permanent".into(),
                enabled: true,
            },
        )
        .await
        .unwrap_err();
        assert!(!crate::client_tunnel_provision::is_router_unreachable_error(&permanent));

        let transient = claim_client_tunnel(
            &reqwest::Client::new(),
            &config,
            ClientTunnelConfig {
                owner_email: "owner@example.com".into(),
                subdomain: "transient".into(),
                enabled: true,
            },
        )
        .await
        .unwrap_err();
        assert!(crate::client_tunnel_provision::is_router_unreachable_error(
            &transient
        ));
        server.abort();
    }

    #[tokio::test]
    async fn client_tunnel_claim_timeout_covers_stalled_error_body() {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 409 Conflict\r\nContent-Type: application/json\r\nContent-Length: 128\r\n\r\n{",
                )
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let mut config = ServerConfig::empty();
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-claim-timeout".into();
        config.router.identity = Some(identity);

        let error = claim_client_tunnel_with_timeout(
            &reqwest::Client::new(),
            &config,
            ClientTunnelConfig {
                owner_email: "owner@example.com".into(),
                subdomain: "claim-timeout".into(),
                enabled: true,
            },
            Duration::from_millis(20),
        )
        .await
        .expect_err("a stalled claim error body must hit the total deadline");

        assert!(crate::client_tunnel_provision::is_router_unreachable_error(
            &error
        ));
        server.abort();
    }

    #[tokio::test]
    async fn subdomain_availability_timeout_covers_stalled_success_body() {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 128\r\n\r\n{",
                )
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });

        let error = check_client_tunnel_subdomain_available_with_timeout(
            &reqwest::Client::new(),
            &format!("http://{addr}"),
            "availability-timeout",
            None,
            Duration::from_millis(20),
        )
        .await
        .expect_err("a stalled availability response must hit the total deadline");

        assert!(error.to_string().contains("availability check timed out"));
        server.abort();
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
    fn lease_signature_is_epoch_scoped() {
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-1".to_string();
        let signature = sign_lease_request(&identity, "share-a", "http", 123, "nonce-1").unwrap();

        let public_key = STANDARD.decode(&identity.public_key).unwrap();
        let public_key: [u8; 32] = public_key.try_into().unwrap();
        let verifying_key = VerifyingKey::from_bytes(&public_key).unwrap();
        let signature = STANDARD.decode(signature).unwrap();
        let signature: [u8; 64] = signature.try_into().unwrap();
        let signature = Signature::from_bytes(&signature);
        let canonical = "namespace-flat-1\ninst-1\nshare-a\nhttp\n123\nnonce-1";

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
        let canonical = "namespace-flat-1\ninst-1\nshare_claim_subdomain\n{\"shareId\":\"share-1\",\"subdomain\":\"share-sub\",\"ownerEmail\":\"owner@example.com\"}\n123\nnonce-1";

        verifying_key
            .verify(canonical.as_bytes(), &signature)
            .unwrap();
    }
}
