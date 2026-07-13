use crate::build_info::BuildInfo;
use crate::domain::settings::config::ServerConfig;
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct HealthResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) config_dir: String,
    pub(in crate::api) web_dist_dir: Option<String>,
    pub(in crate::api) embedded_web_assets: usize,
    pub(in crate::api) unix_ms: u128,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct VersionResponse {
    #[serde(flatten)]
    pub(in crate::api) build: BuildInfo,
    pub(in crate::api) process_id: u32,
    pub(in crate::api) process_instance_id: String,
}

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
    #[serde(default)]
    pub(in crate::api) already_complete: bool,
    #[serde(default)]
    pub(in crate::api) dry_run: bool,
    pub(in crate::api) owner_email: Option<String>,
    pub(in crate::api) router_url: Option<String>,
    pub(in crate::api) client_tunnel_subdomain: Option<String>,
    pub(in crate::api) client_tunnel_claim_status: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(in crate::api) warnings: Vec<String>,
    pub(in crate::api) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api) session_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api) api_token: Option<String>,
}

impl SetupResponse {
    pub(in crate::api) fn from_outcome(outcome: crate::setup::SetupOutcome) -> Self {
        SetupResponse {
            ok: outcome.ok,
            already_complete: outcome.already_complete,
            dry_run: outcome.dry_run,
            owner_email: outcome.owner_email,
            router_url: outcome.router_url,
            client_tunnel_subdomain: outcome.client_tunnel_subdomain,
            client_tunnel_claim_status: outcome.client_tunnel_claim_status,
            warnings: outcome.warnings,
            message: outcome.message,
            session_token: outcome.session_token,
            api_token: outcome.api_token,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct DeleteResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) deleted: bool,
}
