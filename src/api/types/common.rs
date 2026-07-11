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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct DeleteResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) deleted: bool,
}
