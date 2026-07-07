use crate::domain::settings::config::{mask_proxy_url, RouterConfig, ServerConfig};
use serde::Serialize;

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
