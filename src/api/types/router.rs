use crate::clients::router::client::RouterRegisterResult;
use crate::proxy::adapters::AdapterCapability;
use serde::Serialize;

use super::settings::RouterConfigView;

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
pub(in crate::api) struct RouterShareEditPullResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) summary: crate::state::ShareEditSyncSummary,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ProxyCapabilitiesResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) capabilities: Vec<AdapterCapability>,
}
