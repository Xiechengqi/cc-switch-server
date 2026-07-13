use std::net::SocketAddr;

use axum::http::StatusCode;
use serde::Serialize;

use crate::api::error::ApiError;
use crate::api::session::generate_session_token;
use crate::client_tunnel_provision::{
    check_subdomain_for_router, provision_client_tunnel, resolve_setup_subdomain,
    subdomain_conflict_error,
};
use crate::domain::settings::config::{ServerConfig, SetupInput, SetupOptions};
use crate::state::{ServerState, Session};

#[derive(Debug, Clone)]
pub struct SetupExecution {
    pub idempotent: bool,
    pub start_client_tunnel: bool,
    pub issue_session_token: bool,
    pub issue_api_token: bool,
    pub skip_router_claim: bool,
}

impl Default for SetupExecution {
    fn default() -> Self {
        Self {
            idempotent: false,
            start_client_tunnel: true,
            issue_session_token: false,
            issue_api_token: false,
            skip_router_claim: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupOutcome {
    pub ok: bool,
    pub already_complete: bool,
    pub dry_run: bool,
    pub owner_email: Option<String>,
    pub router_url: Option<String>,
    pub client_tunnel_subdomain: Option<String>,
    pub client_tunnel_claim_status: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_token: Option<String>,
}

impl SetupOutcome {
    pub fn already_complete(config: &ServerConfig) -> Self {
        Self {
            ok: true,
            already_complete: true,
            dry_run: false,
            owner_email: config.owner.email.clone(),
            router_url: config.router.url.clone(),
            client_tunnel_subdomain: config.client.tunnel_subdomain.clone(),
            client_tunnel_claim_status: config.client.tunnel_status.clone(),
            warnings: Vec::new(),
            message: "setup is already complete".to_string(),
            session_token: None,
            api_token: None,
        }
    }

    pub fn from_provision(
        config: &ServerConfig,
        claim_status: &str,
        warnings: Vec<String>,
        dry_run: bool,
        message: impl Into<String>,
        session_token: Option<String>,
        api_token: Option<String>,
    ) -> Self {
        Self {
            ok: true,
            already_complete: false,
            dry_run,
            owner_email: config.owner.email.clone(),
            router_url: config.router.url.clone(),
            client_tunnel_subdomain: config.client.tunnel_subdomain.clone(),
            client_tunnel_claim_status: Some(claim_status.to_string()),
            warnings,
            message: message.into(),
            session_token,
            api_token,
        }
    }
}

pub async fn complete_setup(
    state: &ServerState,
    input: SetupInput,
    exec: SetupExecution,
) -> Result<SetupOutcome, ApiError> {
    let options = input.options.clone().unwrap_or_default();
    let dry_run = options.dry_run;
    let allow_offline = options.allow_offline;
    let issue_session_token = exec.issue_session_token || options.issue_session_token;
    let issue_api_token = exec.issue_api_token || options.issue_api_token;

    {
        let config = state.config.read().await;
        if config.is_setup_complete() {
            if exec.idempotent {
                return Ok(SetupOutcome::already_complete(&config));
            }
            return Err(ApiError::conflict_code(
                "setup_already_complete",
                "server setup is already complete",
            ));
        }
    }

    let router_url =
        ServerConfig::preview_router_url(&input.router_url).map_err(ApiError::bad_request)?;
    let tunnel_subdomain =
        resolve_setup_subdomain(state, &router_url, input.client_tunnel_subdomain.as_deref())
            .await?;
    let mut input = input;
    input.client_tunnel_subdomain = Some(tunnel_subdomain);

    let config = ServerConfig::from_setup(input).map_err(map_setup_error)?;

    if dry_run {
        return Ok(SetupOutcome::from_provision(
            &config,
            "dry_run",
            Vec::new(),
            true,
            "setup validation succeeded",
            None,
            None,
        ));
    }

    if let Some(router_url) = config.router_api_base() {
        if let Some(subdomain) = config.client.tunnel_subdomain.clone() {
            match check_subdomain_for_router(state, router_url, &subdomain, None).await {
                Ok(availability) if !availability.available => {
                    return Err(subdomain_conflict_error(
                        &subdomain,
                        availability.reason.as_deref(),
                    ));
                }
                Ok(_) => {}
                Err(error) if allow_offline && error.status == StatusCode::BAD_GATEWAY => {
                    tracing::warn!(
                        error = %error.message,
                        "router subdomain pre-check skipped during setup"
                    );
                }
                Err(error) => return Err(error),
            }
        }
    }

    state
        .replace_config(config.clone())
        .await
        .map_err(ApiError::internal)?;

    let (final_config, claim_status, warnings) = if exec.skip_router_claim {
        (config, "skipped", Vec::new())
    } else {
        let provision = provision_client_tunnel(state, config, allow_offline).await?;
        state
            .replace_config(provision.config.clone())
            .await
            .map_err(ApiError::internal)?;
        (provision.config, provision.claim_status, provision.warnings)
    };

    if let Some(domain) = final_config.router.domain.clone() {
        if let Err(error) = state
            .apply_ui_settings_patch_immediate(serde_json::json!({ "shareRouterDomain": domain }))
            .await
        {
            tracing::warn!(
                error = %error,
                "persist share router domain during setup failed"
            );
        }
    }

    if exec.start_client_tunnel && matches!(claim_status, "claimed" | "skipped") {
        crate::state::start_client_tunnel(state.clone()).await;
    }

    let mut session_token = None;
    if issue_session_token {
        let token = generate_session_token();
        state
            .push_session(Session {
                token: token.clone(),
            })
            .await;
        session_token = Some(token);
    }

    let mut api_token = None;
    if issue_api_token {
        let token = generate_session_token();
        let mut next = state.config.read().await.clone();
        next.set_api_token(&token).map_err(ApiError::internal)?;
        state
            .replace_config(next)
            .await
            .map_err(ApiError::internal)?;
        api_token = Some(token);
    }

    let message = if session_token.is_some() {
        "setup complete; session token issued".to_string()
    } else {
        "setup complete; use password login to enter cc-switch-server".to_string()
    };

    Ok(SetupOutcome::from_provision(
        &final_config,
        claim_status,
        warnings,
        false,
        message,
        session_token,
        api_token,
    ))
}

fn map_setup_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("owner email format is invalid") {
        return ApiError::bad_request_code("invalid_owner_email", message);
    }
    if message.contains("secret must be at least") || message.contains("password") {
        return ApiError::bad_request_code("invalid_password", message);
    }
    if message.contains("router url") {
        return ApiError::bad_request_code("invalid_router_url", message);
    }
    if message.contains("subdomain") {
        return ApiError::bad_request_code("invalid_subdomain", message);
    }
    ApiError::bad_request(message)
}

pub fn log_setup_required_hints(state: &ServerState) {
    let bind_addr = state.bind_addr;
    let config_dir = state.config_dir.display();
    let web_url = setup_web_url(bind_addr);
    let api_url = web_url.clone();
    let example_owner = "owner@example.com";
    let example_password = "password123";
    let example_router = "https://sgptokenswitch.cc";

    tracing::warn!("cc-switch-server setup is required before use");
    tracing::warn!("setup option 1/3: open the web UI in your browser");
    tracing::warn!("  {web_url}");
    tracing::warn!("setup option 2/3: complete setup over HTTP with curl");
    tracing::warn!(
        "  curl -fsS -X POST '{api_url}/api/setup/bootstrap' -H 'content-type: application/json' -d '{{\"password\":\"{example_password}\",\"ownerEmail\":\"{example_owner}\",\"routerUrl\":\"{example_router}\",\"clientTunnelSubdomain\":\"\"}}'"
    );
    tracing::warn!("setup option 3/3: initialize locally with the CLI init subcommand");
    tracing::warn!(
        "  cc-switch-server --config-dir {config_dir} init --owner-email {example_owner} --router-url {example_router} --password {example_password}"
    );
}

pub fn setup_web_url(bind_addr: SocketAddr) -> String {
    let host = if bind_addr.ip().is_unspecified() {
        "127.0.0.1".to_string()
    } else {
        bind_addr.ip().to_string()
    };
    format!("http://{host}:{}", bind_addr.port())
}

pub async fn run_cli_init(cli: &crate::cli::Cli, args: crate::cli::InitArgs) -> anyhow::Result<()> {
    use crate::logging::{LogCapture, RING_BUFFER_CAPACITY};
    use std::sync::Arc;

    if args.password.is_some() && args.password_stdin {
        anyhow::bail!("use either --password or --password-stdin, not both");
    }

    let password = if args.password_stdin {
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .context("read password from stdin")?;
        line.trim_end_matches(['\n', '\r']).to_string()
    } else {
        args.password
            .clone()
            .context("password is required; pass --password or --password-stdin")?
    };

    if password.trim().is_empty() {
        anyhow::bail!("password must not be empty");
    }

    let log_capture = Arc::new(LogCapture::new(RING_BUFFER_CAPACITY));
    let state = crate::state::ServerStateInner::load(cli.clone(), log_capture)
        .context("load server state for init")?;

    let input = SetupInput {
        password,
        owner_email: args.owner_email.clone(),
        router_url: args.router_url.clone(),
        client_tunnel_subdomain: args.client_subdomain.clone(),
        options: Some(SetupOptions {
            dry_run: args.dry_run,
            allow_offline: args.allow_offline,
            issue_session_token: false,
            issue_api_token: false,
        }),
    };

    let outcome = complete_setup(
        &state,
        input,
        SetupExecution {
            idempotent: !args.force,
            start_client_tunnel: false,
            issue_session_token: false,
            issue_api_token: false,
            skip_router_claim: args.skip_router_claim,
        },
    )
    .await
    .map_err(|error| anyhow::anyhow!(error.message))?;

    if outcome.already_complete {
        println!("setup already complete in {}", state.config_dir.display());
        return Ok(());
    }

    if outcome.dry_run {
        println!(
            "setup dry-run ok: subdomain={} router={}",
            outcome
                .client_tunnel_subdomain
                .as_deref()
                .unwrap_or_default(),
            outcome.router_url.as_deref().unwrap_or_default()
        );
        return Ok(());
    }

    println!(
        "setup complete in {}; client subdomain={}",
        state.config_dir.display(),
        outcome
            .client_tunnel_subdomain
            .as_deref()
            .unwrap_or_default()
    );
    if !outcome.warnings.is_empty() {
        for warning in &outcome.warnings {
            println!("warning: {warning}");
        }
    }
    Ok(())
}

use anyhow::Context;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_web_url_uses_localhost_for_unspecified_bind() {
        let addr: SocketAddr = "0.0.0.0:15721".parse().unwrap();
        assert_eq!(setup_web_url(addr), "http://127.0.0.1:15721");
    }
}
