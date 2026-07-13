use axum::http::StatusCode;
use serde::Serialize;

use crate::api::error::ApiError;
use crate::clients::router::client::{self, ClientTunnelConfig, SubdomainAvailability};
use crate::domain::settings::config::ServerConfig;
use crate::domain::subdomain_suggest::{
    self, generate_candidate, is_reserved_subdomain, SUGGEST_MAX_ATTEMPTS,
};
use crate::state::ServerState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SuggestSubdomainOutcome {
    pub subdomain: String,
    pub available: bool,
    pub checked: bool,
    pub attempts: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RouterReachabilityOutcome {
    pub reachable: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ClientTunnelProvisionOutcome {
    pub config: ServerConfig,
    pub claim_status: &'static str,
    pub warnings: Vec<String>,
}

pub(crate) fn is_subdomain_conflict_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("subdomain already claimed")
}

pub(crate) fn is_router_unreachable_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .to_string()
            .to_ascii_lowercase()
            .contains("connection")
    }) || error.to_string().to_ascii_lowercase().contains("timed out")
}

pub(crate) async fn check_subdomain_for_router(
    state: &ServerState,
    router_url: &str,
    subdomain: &str,
    installation_id: Option<&str>,
) -> Result<SubdomainAvailability, ApiError> {
    let http_client = state.http_client().await;
    client::check_client_tunnel_subdomain_available(
        &http_client,
        router_url,
        subdomain,
        installation_id,
    )
    .await
    .map_err(|error| ApiError::bad_gateway(format!("router subdomain check failed: {error}")))
}

pub(crate) async fn check_router_reachable(
    state: &ServerState,
    router_url: &str,
) -> Result<RouterReachabilityOutcome, ApiError> {
    let http_client = state.http_client().await;
    let base = router_url.trim_end_matches('/');
    let url = format!("{base}/v1/healthz");
    let reachable = match http_client.get(url).send().await {
        Ok(response) => response.status().is_success(),
        Err(_) => false,
    };
    Ok(RouterReachabilityOutcome { reachable })
}

pub(crate) async fn suggest_client_tunnel_subdomain(
    state: &ServerState,
    router_url: &str,
    installation_id: Option<&str>,
) -> Result<SuggestSubdomainOutcome, ApiError> {
    let reachability = check_router_reachable(state, router_url).await?;
    if !reachability.reachable {
        return Err(ApiError::bad_gateway("router is unreachable"));
    }

    let mut last_subdomain = String::new();

    for attempt in 0..SUGGEST_MAX_ATTEMPTS {
        let candidate = generate_candidate(&mut rand::thread_rng(), attempt);
        if is_reserved_subdomain(&candidate) {
            continue;
        }
        last_subdomain = candidate.clone();
        let availability =
            check_subdomain_for_router(state, router_url, &candidate, installation_id).await?;
        if availability.available {
            return Ok(SuggestSubdomainOutcome {
                subdomain: candidate,
                available: true,
                checked: true,
                attempts: attempt as u32 + 1,
            });
        }
    }

    Err(subdomain_conflict_error(
        &last_subdomain,
        Some("suggest_exhausted"),
    ))
}

pub(crate) fn generate_memorable_subdomain_fallback() -> String {
    subdomain_suggest::generate_memorable_subdomain(&mut rand::thread_rng())
}

pub(crate) async fn resolve_setup_subdomain(
    state: &ServerState,
    router_url: &str,
    requested: Option<&str>,
) -> Result<String, ApiError> {
    if let Some(value) = requested.map(str::trim).filter(|value| !value.is_empty()) {
        return ServerConfig::preview_client_subdomain(value).map_err(ApiError::bad_request);
    }

    match suggest_client_tunnel_subdomain(state, router_url, None).await {
        Ok(outcome) => Ok(outcome.subdomain),
        Err(error) if error.status == StatusCode::BAD_GATEWAY => {
            Ok(generate_memorable_subdomain_fallback())
        }
        Err(error) => Err(error),
    }
}

pub(crate) async fn provision_client_tunnel(
    state: &ServerState,
    mut config: ServerConfig,
    allow_offline: bool,
) -> Result<ClientTunnelProvisionOutcome, ApiError> {
    let mut warnings = Vec::new();
    let api_base = config
        .router_api_base()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let Some(api_base) = api_base else {
        return Ok(ClientTunnelProvisionOutcome {
            config,
            claim_status: "skipped",
            warnings,
        });
    };

    let http_client = state.http_client().await;
    let installation_id = config
        .router
        .identity
        .as_ref()
        .map(|identity| identity.installation_id.as_str());
    if let Some(subdomain) = config.client.tunnel_subdomain.clone() {
        match check_subdomain_for_router(state, api_base, &subdomain, installation_id).await {
            Ok(availability) if !availability.available => {
                return Err(subdomain_conflict_error(
                    &subdomain,
                    availability.reason.as_deref(),
                ));
            }
            Ok(_) => {}
            Err(error) if allow_offline && error.status == StatusCode::BAD_GATEWAY => {
                warnings.push(format!(
                    "router subdomain pre-check skipped: {}",
                    error.message
                ));
            }
            Err(error) => return Err(error),
        }
    }

    if config.router.identity.is_none() {
        match client::register_installation(&http_client, &mut config).await {
            Ok(_) => {}
            Err(error) if allow_offline && is_router_unreachable_error(&error) => {
                warnings.push(format!(
                    "router installation register skipped (offline): {error}"
                ));
                return Ok(ClientTunnelProvisionOutcome {
                    config,
                    claim_status: "skipped",
                    warnings,
                });
            }
            Err(error) => {
                return Err(ApiError::bad_gateway(format!(
                    "router installation register failed: {error}"
                )));
            }
        }
    }

    if let Some(owner_email) = config.owner.email.as_deref() {
        if let Err(error) = crate::clients::router::email_auth::bind_owner_email_at_setup(
            &http_client,
            &config,
            owner_email,
        )
        .await
        {
            tracing::warn!(
                error = %error.message,
                "router owner bootstrap bind during provision failed"
            );
        }
    }

    let owner_email = config
        .owner
        .email
        .clone()
        .ok_or_else(|| ApiError::bad_request("owner email is not configured"))?;
    let subdomain = config
        .client
        .tunnel_subdomain
        .clone()
        .ok_or_else(|| ApiError::bad_request("client tunnel subdomain is not configured"))?;

    if let Err(error) = crate::state::ensure_router_installation_owner_bound(state, &config).await {
        if allow_offline {
            warnings.push(format!("router owner bind pending: {error}"));
            return Ok(ClientTunnelProvisionOutcome {
                config,
                claim_status: "skipped",
                warnings,
            });
        }
        return Err(ApiError::conflict(error.to_string()));
    }

    match client::claim_client_tunnel(
        &http_client,
        &config,
        ClientTunnelConfig {
            owner_email,
            subdomain: subdomain.clone(),
            enabled: true,
        },
    )
    .await
    {
        Ok(()) => {
            mark_claim_success(state, &mut config).await;
            Ok(ClientTunnelProvisionOutcome {
                config,
                claim_status: "claimed",
                warnings,
            })
        }
        Err(error) if is_subdomain_conflict_error(&error.to_string()) => {
            record_claim_failure(state, &config, error.to_string()).await;
            Err(subdomain_conflict_error(
                &subdomain,
                Some("already_claimed"),
            ))
        }
        Err(error) if allow_offline && is_router_unreachable_error(&error) => {
            warnings.push(format!(
                "router client tunnel claim skipped (offline): {error}"
            ));
            Ok(ClientTunnelProvisionOutcome {
                config,
                claim_status: "skipped",
                warnings,
            })
        }
        Err(error) => {
            record_claim_failure(state, &config, error.to_string()).await;
            Err(ApiError::bad_gateway(format!(
                "router client tunnel claim failed: {error}"
            )))
        }
    }
}

pub(crate) async fn claim_client_tunnel_config(
    state: &ServerState,
    config: &ServerConfig,
) -> Result<(), ApiError> {
    let owner_email = config
        .owner
        .email
        .clone()
        .ok_or_else(|| ApiError::bad_request("owner email is not configured"))?;
    let subdomain = config
        .client
        .tunnel_subdomain
        .clone()
        .ok_or_else(|| ApiError::bad_request("client tunnel subdomain is not configured"))?;
    if config.router.identity.is_none() {
        return Err(ApiError::conflict("router installation is not registered"));
    }
    if let Err(error) = crate::state::ensure_router_installation_owner_bound(state, config).await {
        return Err(ApiError::conflict(error.to_string()));
    }
    let installation_id = config
        .router
        .identity
        .as_ref()
        .map(|identity| identity.installation_id.as_str());
    if let Some(api_base) = config.router_api_base() {
        let availability =
            check_subdomain_for_router(state, api_base, &subdomain, installation_id).await?;
        if !availability.available {
            return Err(subdomain_conflict_error(
                &subdomain,
                availability.reason.as_deref(),
            ));
        }
    }
    let http_client = state.http_client().await;
    client::claim_client_tunnel(
        &http_client,
        config,
        ClientTunnelConfig {
            owner_email,
            subdomain,
            enabled: true,
        },
    )
    .await
    .map_err(map_claim_error)?;
    let mut next = config.clone();
    mark_claim_success(state, &mut next).await;
    Ok(())
}

fn map_claim_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if is_subdomain_conflict_error(&message) {
        return ApiError::conflict(message);
    }
    ApiError::bad_gateway(format!("router client tunnel claim failed: {error}"))
}

pub(crate) fn subdomain_conflict_error(subdomain: &str, reason: Option<&str>) -> ApiError {
    let detail = reason.unwrap_or("already_claimed");
    ApiError::conflict_code(
        "client_tunnel_subdomain_conflict",
        format!(
            "client_tunnel_subdomain_conflict: subdomain '{subdomain}' is unavailable ({detail})"
        ),
    )
}

pub(crate) async fn mark_claim_success(state: &ServerState, config: &mut ServerConfig) {
    config.client.tunnel_status = Some("claimed_remote".to_string());
    config.router.last_register_error = None;
    if let Err(error) = state.replace_config(config.clone()).await {
        tracing::warn!(error = %error, "persist client tunnel claim success failed");
    }
    {
        let mut shares = state.shares.write().await;
        shares.router_registered = true;
        shares.last_router_error = None;
    }
    if let Err(error) = state.save_shares().await {
        tracing::warn!(error = %error, "persist router registered flag failed");
    }
}

async fn record_claim_failure(state: &ServerState, config: &ServerConfig, message: String) {
    let mut next = config.clone();
    next.client.tunnel_status = Some("claim_failed".to_string());
    next.router.last_register_error = Some(message.clone());
    if let Err(error) = state.replace_config(next).await {
        tracing::warn!(error = %error, "persist client tunnel claim failure failed");
    }
    {
        let mut shares = state.shares.write().await;
        shares.router_registered = false;
        shares.last_router_error = Some(message);
    }
    if let Err(error) = state.save_shares().await {
        tracing::warn!(error = %error, "persist router claim failure flag failed");
    }
}

pub(crate) fn derive_client_tunnel_claim_status(
    config: &ServerConfig,
    last_router_error: Option<&str>,
) -> &'static str {
    if matches!(
        config.client.tunnel_status.as_deref(),
        Some("claimed_remote" | "connected" | "active" | "running")
    ) && last_router_error.is_none()
    {
        return "claimed";
    }
    if config.client.tunnel_status.as_deref() == Some("claim_failed")
        || last_router_error.is_some_and(is_subdomain_conflict_error)
    {
        return "conflict";
    }
    if last_router_error.is_some() {
        return "error";
    }
    "unclaimed"
}

pub(crate) fn derive_client_tunnel_connectivity_status(
    runtime_status: Option<&str>,
    runtime_error: Option<&str>,
    claim_status: &str,
) -> &'static str {
    if claim_status == "unclaimed" || claim_status == "conflict" || claim_status == "error" {
        return "disconnected";
    }
    if let Some(status) = runtime_status {
        if matches!(status, "connected" | "running" | "active" | "renewing") {
            return "connected";
        }
        if status == "renewal_retrying" || status == "retrying" {
            return "connecting";
        }
    }
    if runtime_error.is_some() {
        return "connecting";
    }
    "disconnected"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::settings::config::ServerConfig;

    #[test]
    fn derive_claim_status_marks_conflict_from_router_error() {
        let mut config = ServerConfig::empty();
        config.client.tunnel_subdomain = Some("us01".to_string());
        let status = derive_client_tunnel_claim_status(
            &config,
            Some("router client tunnel claim failed: 409 Conflict: subdomain already claimed"),
        );
        assert_eq!(status, "conflict");
    }

    #[test]
    fn derive_connectivity_requires_claim_before_connected() {
        assert_eq!(
            derive_client_tunnel_connectivity_status(Some("connected"), None, "conflict"),
            "disconnected"
        );
        assert_eq!(
            derive_client_tunnel_connectivity_status(Some("connected"), None, "claimed"),
            "connected"
        );
    }
}
