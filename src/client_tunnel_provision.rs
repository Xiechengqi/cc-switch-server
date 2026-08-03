use std::time::Duration;

use anyhow::Context;
use axum::http::StatusCode;
use serde::Serialize;

use crate::api::error::ApiError;
use crate::clients::router::client::{
    self, ClientTunnelClaimError, ClientTunnelConfig, ClientTunnelView, SubdomainAvailability,
};
use crate::domain::settings::config::{ClientTunnelClaimIntent, ServerConfig};
use crate::domain::subdomain_suggest::{
    self, generate_candidate, is_reserved_subdomain, SUGGEST_MAX_ATTEMPTS,
};
use crate::state::ServerState;

const CLIENT_TUNNEL_RECONCILE_TIMEOUT: Duration = Duration::from_secs(10);

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
    pub subdomain_check_supported: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct SubdomainCheckOutcome {
    pub available: bool,
    pub checked: bool,
    pub reason: Option<String>,
}

pub(crate) fn is_subdomain_availability_api_missing_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("404")
        || lower.contains("unregistered-subdomain")
        || lower.contains("subdomain availability check failed") && lower.contains("not found")
}

pub(crate) fn is_subdomain_availability_api_missing_error(error: &anyhow::Error) -> bool {
    is_subdomain_availability_api_missing_message(&error.to_string())
}

fn is_subdomain_availability_api_missing_api_error(error: &ApiError) -> bool {
    error.status == StatusCode::BAD_GATEWAY
        && is_subdomain_availability_api_missing_message(&error.message)
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
    if let Some(error) = error.downcast_ref::<crate::state::RouterRegistrationFailure>() {
        return error.is_unreachable();
    }
    if error
        .downcast_ref::<crate::state::RouterRegistrationTimeout>()
        .is_some()
    {
        return true;
    }
    error.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|error| error.is_connect() || error.is_timeout())
            || cause
                .downcast_ref::<crate::clients::router::client::RegisterInstallationAttemptError>()
                .is_some_and(|error| error.is_transient())
            || cause
                .downcast_ref::<crate::clients::router::client::ClientTunnelClaimError>()
                .is_some_and(|error| error.is_transient())
            || cause
                .downcast_ref::<crate::state::RouterRegistrationTimeout>()
                .is_some()
    })
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

pub(crate) async fn check_subdomain_for_router_outcome(
    state: &ServerState,
    router_url: &str,
    subdomain: &str,
    installation_id: Option<&str>,
) -> Result<SubdomainCheckOutcome, ApiError> {
    match check_subdomain_for_router(state, router_url, subdomain, installation_id).await {
        Ok(availability) => Ok(SubdomainCheckOutcome {
            available: availability.available,
            checked: true,
            reason: availability.reason,
        }),
        Err(error) if is_subdomain_availability_api_missing_api_error(&error) => {
            Ok(SubdomainCheckOutcome {
                available: true,
                checked: false,
                reason: Some("router_subdomain_api_unavailable".to_string()),
            })
        }
        Err(error) => Err(error),
    }
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
    let subdomain_check_supported = if reachable {
        match client::check_client_tunnel_subdomain_available(
            &http_client,
            router_url,
            "zzzzprobe",
            None,
        )
        .await
        {
            Ok(_) => true,
            Err(error) => !is_subdomain_availability_api_missing_error(&error),
        }
    } else {
        false
    };
    Ok(RouterReachabilityOutcome {
        reachable,
        subdomain_check_supported,
    })
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
    let availability_api_supported = reachability.subdomain_check_supported;

    for attempt in 0..SUGGEST_MAX_ATTEMPTS {
        let candidate = generate_candidate(&mut rand::thread_rng(), attempt);
        if is_reserved_subdomain(&candidate) {
            continue;
        }
        last_subdomain = candidate.clone();

        if !availability_api_supported {
            return Ok(SuggestSubdomainOutcome {
                subdomain: candidate,
                available: false,
                checked: false,
                attempts: attempt as u32 + 1,
            });
        }

        match check_subdomain_for_router(state, router_url, &candidate, installation_id).await {
            Ok(availability) if availability.available => {
                return Ok(SuggestSubdomainOutcome {
                    subdomain: candidate,
                    available: true,
                    checked: true,
                    attempts: attempt as u32 + 1,
                });
            }
            Ok(_) => {}
            Err(error) if is_subdomain_availability_api_missing_api_error(&error) => {
                return Ok(SuggestSubdomainOutcome {
                    subdomain: candidate,
                    available: false,
                    checked: false,
                    attempts: attempt as u32 + 1,
                });
            }
            Err(error) => return Err(error),
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
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let Some(api_base) = api_base else {
        return Ok(ClientTunnelProvisionOutcome {
            config,
            claim_status: "skipped",
            warnings,
        });
    };

    let installation_id = config
        .registered_router_identity()
        .map(|identity| identity.installation_id.as_str());
    if let Some(subdomain) = config.client.tunnel_subdomain.clone() {
        match check_subdomain_for_router(state, &api_base, &subdomain, installation_id).await {
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

    if !config.has_registered_router_identity() {
        match state.register_router_installation().await {
            Ok(_) => {}
            Err(error) if allow_offline && is_router_unreachable_error(&error) => {
                if let Err(save_error) = state.mark_client_tunnel_claim_skipped().await {
                    tracing::warn!(error = %save_error, "persist claim_skipped status failed");
                }
                config = state.config_snapshot().await;
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

    match claim_client_tunnel_config(state).await {
        Ok(claimed) => {
            config = claimed;
            Ok(ClientTunnelProvisionOutcome {
                config,
                claim_status: "claimed",
                warnings,
            })
        }
        Err(error) if allow_offline && error.status == StatusCode::BAD_GATEWAY => {
            if let Err(save_error) = state.mark_client_tunnel_claim_skipped().await {
                tracing::warn!(error = %save_error, "persist claim_skipped status failed");
            }
            config = state.config_snapshot().await;
            warnings.push(format!(
                "router client tunnel claim skipped (offline): {}",
                error.message
            ));
            Ok(ClientTunnelProvisionOutcome {
                config,
                claim_status: "skipped",
                warnings,
            })
        }
        Err(error) => Err(error),
    }
}

pub(crate) async fn claim_client_tunnel_config(
    state: &ServerState,
) -> Result<ServerConfig, ApiError> {
    let _claim = state.lock_client_tunnel_claim().await;
    let mut config = state.config_snapshot().await;
    if !config.has_registered_router_identity() {
        state
            .register_router_installation()
            .await
            .map_err(|error| {
                ApiError::bad_gateway(format!("router installation register failed: {error}"))
            })?;
        config = state.config_snapshot().await;
    }
    let intent = ClientTunnelClaimIntent::from_config(&config).map_err(ApiError::bad_request)?;
    if config.client.claim_pending.as_ref() == Some(&intent) {
        match reconcile_remote_client_tunnel(state, &config, &intent).await {
            Ok(true) => return finish_reconciled_client_tunnel_claim(state, &intent).await,
            Ok(false) => {
                commit_claim_failure(
                    state,
                    &intent,
                    false,
                    "pending client tunnel claim was not found on the router".to_string(),
                )
                .await?;
                config = state.config_snapshot().await;
            }
            Err(error) => {
                state
                    .retain_client_tunnel_claim_pending(&intent, error.to_string())
                    .await
                    .map_err(ApiError::internal)?;
                return Err(ApiError::bad_gateway(format!(
                    "router client tunnel claim reconciliation failed: {error}"
                )));
            }
        }
    }
    if config.client.claim_pending.is_none()
        && matches!(
            config.client.tunnel_status.as_deref(),
            Some("claimed_remote" | "connected" | "active" | "running")
        )
        && reconcile_remote_client_tunnel(state, &config, &intent)
            .await
            .unwrap_or(false)
    {
        return Ok(config);
    }
    config = state
        .begin_client_tunnel_claim(&intent)
        .await
        .map_err(ApiError::internal)?;
    if let Err(error) = crate::state::ensure_router_installation_owner_bound(state, &config).await {
        commit_claim_failure(state, &intent, false, error.to_string()).await?;
        return Err(ApiError::conflict(error.to_string()));
    }
    let http_client = state.http_client().await;
    let result = client::claim_client_tunnel(
        &http_client,
        &config,
        ClientTunnelConfig {
            owner_email: intent.owner_email.clone(),
            subdomain: intent.subdomain.clone(),
            enabled: true,
        },
    )
    .await;

    match result {
        Ok(()) => finish_client_tunnel_claim(state, &config, &intent).await,
        Err(error) if claim_outcome_is_uncertain(&error) => {
            match reconcile_remote_client_tunnel(state, &config, &intent).await {
                Ok(true) => finish_reconciled_client_tunnel_claim(state, &intent).await,
                Ok(false) => {
                    let conflict = is_typed_claim_conflict(&error);
                    commit_claim_failure(state, &intent, conflict, error.to_string()).await?;
                    Err(map_claim_error(error))
                }
                Err(reconcile_error) => {
                    let message = format!(
                        "{}; router client tunnel reconciliation failed: {reconcile_error}",
                        error
                    );
                    state
                        .retain_client_tunnel_claim_pending(&intent, message)
                        .await
                        .map_err(ApiError::internal)?;
                    Err(map_claim_error(error))
                }
            }
        }
        Err(error) => {
            commit_claim_failure(state, &intent, false, error.to_string()).await?;
            Err(map_claim_error(error))
        }
    }
}

#[cfg(test)]
mod transient_classification_tests {
    use super::*;
    use crate::clients::router::client::{
        ClientTunnelClaimError, RegisterInstallationAttemptError,
    };

    #[test]
    fn typed_registration_statuses_do_not_depend_on_response_wording() {
        let permanent = anyhow::Error::new(RegisterInstallationAttemptError::Rejected {
            status: reqwest::StatusCode::BAD_REQUEST,
            body: "connection policy is invalid".to_string(),
        });
        assert!(!is_router_unreachable_error(&permanent));

        for status in [
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            reqwest::StatusCode::GATEWAY_TIMEOUT,
        ] {
            let transient = anyhow::Error::new(RegisterInstallationAttemptError::Rejected {
                status,
                body: "retry later".to_string(),
            });
            assert!(is_router_unreachable_error(&transient), "{status}");
        }
    }

    #[test]
    fn typed_client_tunnel_claim_statuses_distinguish_permanent_and_transient() {
        let permanent = anyhow::Error::new(ClientTunnelClaimError::Rejected {
            status: reqwest::StatusCode::CONFLICT,
            body: "connection already belongs to another owner".to_string(),
        });
        assert!(!is_router_unreachable_error(&permanent));

        let transient = anyhow::Error::new(ClientTunnelClaimError::Rejected {
            status: reqwest::StatusCode::SERVICE_UNAVAILABLE,
            body: "retry later".to_string(),
        });
        assert!(is_router_unreachable_error(&transient));
    }
}

fn map_claim_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if is_typed_claim_conflict(&error) {
        return ApiError::conflict_code("client_tunnel_subdomain_conflict", message);
    }
    ApiError::bad_gateway(format!("router client tunnel claim failed: {error}"))
}

fn typed_claim_error(error: &anyhow::Error) -> Option<&ClientTunnelClaimError> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<ClientTunnelClaimError>())
}

fn is_typed_claim_conflict(error: &anyhow::Error) -> bool {
    typed_claim_error(error).is_some_and(ClientTunnelClaimError::is_conflict)
}

fn claim_outcome_is_uncertain(error: &anyhow::Error) -> bool {
    typed_claim_error(error).is_some_and(ClientTunnelClaimError::outcome_is_uncertain)
}

fn remote_tunnel_matches_intent(
    remote: Option<&ClientTunnelView>,
    intent: &ClientTunnelClaimIntent,
) -> bool {
    remote.is_some_and(|remote| {
        remote.owner_email.eq_ignore_ascii_case(&intent.owner_email)
            && remote.subdomain == intent.subdomain
            && remote.enabled
    })
}

async fn reconcile_remote_client_tunnel(
    state: &ServerState,
    config: &ServerConfig,
    intent: &ClientTunnelClaimIntent,
) -> anyhow::Result<bool> {
    let http_client = state.http_client().await;
    let remote = tokio::time::timeout(
        CLIENT_TUNNEL_RECONCILE_TIMEOUT,
        client::get_client_tunnel(&http_client, config),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "router client tunnel reconciliation timed out after {}s",
            CLIENT_TUNNEL_RECONCILE_TIMEOUT.as_secs_f64()
        )
    })??;
    Ok(remote_tunnel_matches_intent(remote.as_ref(), intent))
}

async fn finish_client_tunnel_claim(
    state: &ServerState,
    request_config: &ServerConfig,
    intent: &ClientTunnelClaimIntent,
) -> Result<ServerConfig, ApiError> {
    match state.commit_client_tunnel_claim_success(intent).await {
        Ok(config) => complete_client_tunnel_claim(state, config).await,
        Err(commit_error) => {
            tracing::error!(error = %commit_error, "router tunnel was claimed remotely but local state commit failed");
            match reconcile_remote_client_tunnel(state, request_config, intent).await {
                Ok(true) => finish_reconciled_client_tunnel_claim(state, intent).await,
                Ok(false) => {
                    let message = format!(
                        "router tunnel was claimed remotely but local state commit failed: {commit_error}; remote reconciliation did not match the pending claim"
                    );
                    commit_claim_failure(state, intent, false, message).await?;
                    Err(ApiError::internal(commit_error))
                }
                Err(reconcile_error) => {
                    let message = format!(
                        "router tunnel was claimed remotely but local state commit failed: {commit_error}; reconciliation failed: {reconcile_error}"
                    );
                    state
                        .retain_client_tunnel_claim_pending(intent, message)
                        .await
                        .map_err(ApiError::internal)?;
                    Err(ApiError::internal(commit_error))
                }
            }
        }
    }
}

async fn finish_reconciled_client_tunnel_claim(
    state: &ServerState,
    intent: &ClientTunnelClaimIntent,
) -> Result<ServerConfig, ApiError> {
    let config = state
        .commit_client_tunnel_claim_success(intent)
        .await
        .map_err(ApiError::internal)?;
    complete_client_tunnel_claim(state, config).await
}

async fn complete_client_tunnel_claim(
    state: &ServerState,
    _config: ServerConfig,
) -> Result<ServerConfig, ApiError> {
    state
        .complete_router_registration_control_plane("client_tunnel_claim")
        .await
        .context("persist router control-plane state after client tunnel claim")
        .map_err(ApiError::internal)?;
    state.deliver_setup_completion_after_claim().await;
    Ok(state.config_snapshot().await)
}

async fn commit_claim_failure(
    state: &ServerState,
    intent: &ClientTunnelClaimIntent,
    conflict: bool,
    message: String,
) -> Result<(), ApiError> {
    state
        .commit_client_tunnel_claim_failure(intent, conflict, message.clone())
        .await
        .map_err(ApiError::internal)?;
    state
        .mutate_shares_immediate(|shares| {
            shares.router_registered = false;
            shares.last_router_error = Some(message);
        })
        .await
        .map_err(ApiError::internal)?;
    Ok(())
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

pub(crate) async fn reconcile_pending_client_tunnel_claim(
    state: &ServerState,
) -> anyhow::Result<bool> {
    let _claim = state.lock_client_tunnel_claim().await;
    let config = state.config_snapshot().await;
    let Some(intent) = config.client.claim_pending.clone() else {
        return Ok(false);
    };
    if !intent.matches_config(&config) {
        anyhow::bail!("pending client tunnel claim no longer matches current configuration");
    }
    match reconcile_remote_client_tunnel(state, &config, &intent).await {
        Ok(true) => {
            state.commit_client_tunnel_claim_success(&intent).await?;
            state
                .complete_router_registration_control_plane("client_tunnel_claim_reconcile")
                .await?;
            state.deliver_setup_completion_after_claim().await;
            Ok(true)
        }
        Ok(false) => {
            commit_claim_failure(
                state,
                &intent,
                false,
                "pending client tunnel claim was not found on the router".to_string(),
            )
            .await
            .map_err(|error| anyhow::anyhow!(error.message))?;
            Ok(false)
        }
        Err(error) => {
            state
                .retain_client_tunnel_claim_pending(&intent, error.to_string())
                .await?;
            Err(error)
        }
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
    if config.client.tunnel_status.as_deref() == Some("claim_conflict")
        || last_router_error.is_some_and(is_subdomain_conflict_error)
    {
        return "conflict";
    }
    if config.client.tunnel_status.as_deref() == Some("claim_failed") {
        return "error";
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
    use std::net::{IpAddr, Ipv4Addr};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::extract::State as AxumState;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use serde_json::{json, Value};

    use super::*;
    use crate::cli::Cli;
    use crate::domain::settings::config::ServerConfig;
    use crate::domain::sharing::shares::ShareStore;
    use crate::logging::{LogCapture, RING_BUFFER_CAPACITY};
    use crate::state::ServerStateInner;

    fn test_state(name: &str) -> ServerState {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let config_dir =
            std::env::temp_dir().join(format!("cc-switch-client-tunnel-{name}-{nanos}"));
        test_state_at(config_dir)
    }

    fn test_state_at(config_dir: PathBuf) -> ServerState {
        ServerStateInner::load(
            Cli {
                host: IpAddr::V4(Ipv4Addr::LOCALHOST),
                port: 0,
                config_dir: Some(config_dir),
                web_dist_dir: None,
                log_level: "warn".to_string(),
                command: None,
            },
            Arc::new(LogCapture::new(RING_BUFFER_CAPACITY)),
        )
        .unwrap()
    }

    async fn configure_claim_state(
        state: &ServerState,
        router_url: String,
        installation_id: &str,
        subdomain: &str,
    ) -> ClientTunnelClaimIntent {
        let mut config = ServerConfig::empty();
        config.router.url = Some(router_url);
        let mut identity = client::generate_identity_without_installation();
        identity.installation_id = installation_id.to_string();
        config.router.identity = Some(identity);
        config.owner.email = Some("owner@example.com".to_string());
        config.client.tunnel_subdomain = Some(subdomain.to_string());
        state.replace_config(config.clone()).await.unwrap();
        ClientTunnelClaimIntent::from_config(&config).unwrap()
    }

    async fn owner_email_handler() -> Json<Value> {
        Json(json!({
            "ok": true,
            "ownerEmail": "owner@example.com",
            "ownerVerified": true
        }))
    }

    #[test]
    fn detects_missing_subdomain_availability_api_from_router_proxy_error() {
        let message =
            "router subdomain availability check failed: 404 Not Found: unregistered-subdomain";
        assert!(is_subdomain_availability_api_missing_message(message));
    }

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

    #[test]
    fn typed_claim_status_classification_does_not_depend_on_body() {
        let conflict = anyhow::Error::new(ClientTunnelClaimError::Rejected {
            status: reqwest::StatusCode::CONFLICT,
            body: "arbitrary router response".to_string(),
        });
        assert!(is_typed_claim_conflict(&conflict));
        assert!(claim_outcome_is_uncertain(&conflict));
        assert_eq!(map_claim_error(conflict).status, StatusCode::CONFLICT);

        let server_error = anyhow::Error::new(ClientTunnelClaimError::Rejected {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: "arbitrary router response".to_string(),
        });
        assert!(!is_typed_claim_conflict(&server_error));
        assert!(!claim_outcome_is_uncertain(&server_error));
        assert_eq!(
            map_claim_error(server_error).status,
            StatusCode::BAD_GATEWAY
        );

        let timeout = anyhow::Error::new(ClientTunnelClaimError::Timeout {
            timeout_seconds: 0.01,
        });
        assert!(claim_outcome_is_uncertain(&timeout));
        assert_eq!(map_claim_error(timeout).status, StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn concurrent_claim_is_singleflight_and_preserves_heartbeat_and_config_updates() {
        #[derive(Clone)]
        struct Gate {
            claims: Arc<AtomicUsize>,
            received: Arc<tokio::sync::Notify>,
            release: Arc<tokio::sync::Notify>,
        }

        async fn claim_handler(
            AxumState(gate): AxumState<Gate>,
            Json(_request): Json<Value>,
        ) -> Json<Value> {
            gate.claims.fetch_add(1, Ordering::SeqCst);
            gate.received.notify_one();
            gate.release.notified().await;
            Json(json!({"ok": true}))
        }

        async fn get_handler() -> Json<Value> {
            Json(json!({
                "tunnel": {
                    "ownerEmail": "owner@example.com",
                    "subdomain": "claim-singleflight",
                    "enabled": true
                }
            }))
        }

        let gate = Gate {
            claims: Arc::new(AtomicUsize::new(0)),
            received: Arc::new(tokio::sync::Notify::new()),
            release: Arc::new(tokio::sync::Notify::new()),
        };
        let app = Router::new()
            .route("/v1/installations/owner-email", get(owner_email_handler))
            .route("/v1/installations/client-tunnel/claim", post(claim_handler))
            .route("/v1/installations/client-tunnel", get(get_handler))
            .with_state(gate.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state("claim-singleflight");
        let config_dir = state.config_dir.clone();
        configure_claim_state(
            &state,
            format!("http://{addr}"),
            "inst-singleflight",
            "claim-singleflight",
        )
        .await;
        let mut concurrent_config = state.config_snapshot().await;

        let first_state = state.clone();
        let first = tokio::spawn(async move { claim_client_tunnel_config(&first_state).await });
        let second_state = state.clone();
        let second = tokio::spawn(async move { claim_client_tunnel_config(&second_state).await });
        gate.received.notified().await;
        concurrent_config.upgrade_policy.auto_upgrade_enabled = true;
        state.replace_config(concurrent_config).await.unwrap();
        state.record_client_tunnel_heartbeat(4242).await.unwrap();
        gate.release.notify_one();

        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
        let config = state.config_snapshot().await;
        assert_eq!(gate.claims.load(Ordering::SeqCst), 1);
        assert_eq!(config.client.last_heartbeat_ms, Some(4242));
        assert!(config.upgrade_policy.auto_upgrade_enabled);
        assert_eq!(
            config.client.tunnel_status.as_deref(),
            Some("claimed_remote")
        );
        assert!(config.client.claim_pending.is_none());

        server.abort();
        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[tokio::test]
    async fn conflict_and_server_error_commit_distinct_claim_states() {
        async fn conflict_handler() -> (StatusCode, &'static str) {
            (StatusCode::CONFLICT, "arbitrary ownership response")
        }
        async fn server_error_handler() -> (StatusCode, &'static str) {
            (StatusCode::INTERNAL_SERVER_ERROR, "arbitrary failure")
        }
        async fn no_remote_tunnel() -> Json<Value> {
            Json(json!({"tunnel": null}))
        }

        for (name, claim_route, expected_status, expected_state) in [
            (
                "claim-conflict",
                post(conflict_handler),
                StatusCode::CONFLICT,
                "claim_conflict",
            ),
            (
                "claim-server-error",
                post(server_error_handler),
                StatusCode::BAD_GATEWAY,
                "claim_failed",
            ),
        ] {
            let app = Router::new()
                .route("/v1/installations/owner-email", get(owner_email_handler))
                .route("/v1/installations/client-tunnel/claim", claim_route)
                .route("/v1/installations/client-tunnel", get(no_remote_tunnel));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let state = test_state(name);
            let config_dir = state.config_dir.clone();
            configure_claim_state(
                &state,
                format!("http://{addr}"),
                &format!("inst-{name}"),
                name,
            )
            .await;

            let error = claim_client_tunnel_config(&state).await.unwrap_err();
            assert_eq!(error.status, expected_status);
            let config = state.config_snapshot().await;
            assert_eq!(config.client.tunnel_status.as_deref(), Some(expected_state));
            assert!(config.client.claim_pending.is_none());

            server.abort();
            drop(state);
            std::fs::remove_dir_all(config_dir).unwrap();
        }
    }

    #[tokio::test]
    async fn local_claim_commit_failure_reconciles_matching_remote_state() {
        #[derive(Clone)]
        struct Counts {
            claims: Arc<AtomicUsize>,
            gets: Arc<AtomicUsize>,
        }
        async fn claim_handler(AxumState(counts): AxumState<Counts>) -> Json<Value> {
            counts.claims.fetch_add(1, Ordering::SeqCst);
            Json(json!({"ok": true}))
        }
        async fn get_handler(AxumState(counts): AxumState<Counts>) -> Json<Value> {
            counts.gets.fetch_add(1, Ordering::SeqCst);
            Json(json!({
                "tunnel": {
                    "ownerEmail": "owner@example.com",
                    "subdomain": "commit-reconcile",
                    "enabled": true
                }
            }))
        }

        let counts = Counts {
            claims: Arc::new(AtomicUsize::new(0)),
            gets: Arc::new(AtomicUsize::new(0)),
        };
        let app = Router::new()
            .route("/v1/installations/owner-email", get(owner_email_handler))
            .route("/v1/installations/client-tunnel/claim", post(claim_handler))
            .route("/v1/installations/client-tunnel", get(get_handler))
            .with_state(counts.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state("commit-reconcile");
        let config_dir = state.config_dir.clone();
        configure_claim_state(
            &state,
            format!("http://{addr}"),
            "inst-commit-reconcile",
            "commit-reconcile",
        )
        .await;
        state.fail_next_client_tunnel_claim_commit();

        claim_client_tunnel_config(&state).await.unwrap();

        let config = state.config_snapshot().await;
        assert_eq!(counts.claims.load(Ordering::SeqCst), 1);
        assert_eq!(counts.gets.load(Ordering::SeqCst), 1);
        assert_eq!(
            config.client.tunnel_status.as_deref(),
            Some("claimed_remote")
        );
        assert!(config.client.claim_pending.is_none());

        server.abort();
        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[tokio::test]
    async fn pending_claim_reconciles_explicitly_and_after_restart() {
        async fn matching_remote_tunnel() -> Json<Value> {
            Json(json!({
                "tunnel": {
                    "ownerEmail": "owner@example.com",
                    "subdomain": "pending-reconcile",
                    "enabled": true
                }
            }))
        }

        let app = Router::new().route(
            "/v1/installations/client-tunnel",
            get(matching_remote_tunnel),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let explicit = test_state("explicit-pending-reconcile");
        let explicit_dir = explicit.config_dir.clone();
        let intent = configure_claim_state(
            &explicit,
            format!("http://{addr}"),
            "inst-explicit-reconcile",
            "pending-reconcile",
        )
        .await;
        explicit.begin_client_tunnel_claim(&intent).await.unwrap();
        assert!(reconcile_pending_client_tunnel_claim(&explicit)
            .await
            .unwrap());
        assert!(explicit
            .config_snapshot()
            .await
            .client
            .claim_pending
            .is_none());
        drop(explicit);
        std::fs::remove_dir_all(explicit_dir).unwrap();

        let before_restart = test_state("startup-pending-reconcile");
        let restart_dir = before_restart.config_dir.clone();
        let intent = configure_claim_state(
            &before_restart,
            format!("http://{addr}"),
            "inst-startup-reconcile",
            "pending-reconcile",
        )
        .await;
        before_restart
            .begin_client_tunnel_claim(&intent)
            .await
            .unwrap();
        drop(before_restart);

        let restarted = test_state_at(restart_dir.clone());
        crate::state::restore_tunnels(restarted.clone()).await;
        let config = restarted.config_snapshot().await;
        assert!(config.client.claim_pending.is_none());
        assert_eq!(
            config.client.tunnel_status.as_deref(),
            Some("claimed_remote")
        );

        server.abort();
        drop(restarted);
        std::fs::remove_dir_all(restart_dir).unwrap();
    }

    #[tokio::test]
    async fn record_claim_failure_commits_share_router_state_immediately() {
        let state = test_state("claim-failure");
        let config_dir = state.config_dir.clone();
        let message = "router claim rejected".to_string();
        let mut config = ServerConfig::empty();
        config.router.url = Some("https://router.example.com".to_string());
        let mut identity = client::generate_identity_without_installation();
        identity.installation_id = "inst-claim-failure".to_string();
        config.router.identity = Some(identity);
        config.owner.email = Some("owner@example.com".to_string());
        config.client.tunnel_subdomain = Some("claim-failure".to_string());
        state.replace_config(config.clone()).await.unwrap();
        let intent = ClientTunnelClaimIntent::from_config(&config).unwrap();
        state.begin_client_tunnel_claim(&intent).await.unwrap();

        commit_claim_failure(&state, &intent, false, message.clone())
            .await
            .unwrap();

        let shares = state.shares.read().await.clone();
        assert!(!shares.router_registered);
        assert_eq!(shares.last_router_error.as_deref(), Some(message.as_str()));
        let persisted = ShareStore::load_or_default(&config_dir).unwrap();
        assert!(!persisted.router_registered);
        assert_eq!(
            persisted.last_router_error.as_deref(),
            Some(message.as_str())
        );

        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[tokio::test]
    async fn pending_claim_is_invalidated_when_its_configuration_fingerprint_changes() {
        let state = test_state("pending-fingerprint-change");
        let config_dir = state.config_dir.clone();
        let intent = configure_claim_state(
            &state,
            "https://router.example.com".to_string(),
            "inst-pending-fingerprint",
            "pending-fingerprint",
        )
        .await;
        state.begin_client_tunnel_claim(&intent).await.unwrap();

        let mut changed = state.config_snapshot().await;
        changed.owner.email = Some("replacement@example.com".to_string());
        state.replace_config(changed).await.unwrap();

        let config = state.config_snapshot().await;
        assert!(config.client.claim_pending.is_none());
        assert_eq!(config.client.tunnel_status.as_deref(), Some("claim_failed"));
        assert!(config
            .router
            .last_register_error
            .as_deref()
            .is_some_and(|message| message.contains("invalidated")));

        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }
}
