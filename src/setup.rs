use std::net::SocketAddr;

use axum::http::StatusCode;
use rand::RngCore;
use serde::Serialize;

use crate::api::error::ApiError;
use crate::api::session::generate_session_token;
use crate::client_tunnel_provision::{
    check_subdomain_for_router, provision_client_tunnel, resolve_setup_subdomain,
    subdomain_conflict_error,
};
use crate::domain::settings::config::{
    ServerConfig, SetupCompletionNotificationState, SetupCompletionNotificationStatus, SetupInput,
    SetupOptions,
};
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
    pub setup_completion_notification_status: Option<String>,
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
            setup_completion_notification_status: setup_completion_notification_status(config),
            warnings: setup_completion_notification_warning(config)
                .into_iter()
                .collect(),
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
        let mut warnings = warnings;
        if let Some(warning) = setup_completion_notification_warning(config) {
            warnings.push(warning);
        }
        Self {
            ok: true,
            already_complete: false,
            dry_run,
            owner_email: config.owner.email.clone(),
            router_url: config.router.url.clone(),
            client_tunnel_subdomain: config.client.tunnel_subdomain.clone(),
            client_tunnel_claim_status: Some(claim_status.to_string()),
            setup_completion_notification_status: setup_completion_notification_status(config),
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
    let _setup_flight = state.lock_setup().await;
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
    let setup_completion_notification = if !dry_run && !exec.skip_router_claim {
        Some(SetupCompletionNotificationState::new(
            generate_setup_id(),
            password_hint(&input.password),
            now_ms_i64(),
        ))
    } else {
        None
    };

    let mut config = ServerConfig::from_setup(input).map_err(map_setup_error)?;
    config.setup_completion_notification = setup_completion_notification;

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

fn generate_setup_id() -> String {
    let mut bytes = [0_u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn password_hint(password: &str) -> String {
    let first = password
        .chars()
        .next()
        .filter(char::is_ascii_alphanumeric)
        .unwrap_or('*');
    let last = password
        .chars()
        .next_back()
        .filter(char::is_ascii_alphanumeric)
        .unwrap_or('*');
    format!("{first}******{last}")
}

fn setup_completion_notification_status(config: &ServerConfig) -> Option<String> {
    config
        .setup_completion_notification
        .as_ref()
        .map(|notification| notification.status.as_str().to_string())
}

fn setup_completion_notification_warning(config: &ServerConfig) -> Option<String> {
    let notification = config.setup_completion_notification.as_ref()?;
    match notification.status {
        SetupCompletionNotificationStatus::WaitingForClaim => Some(
            "setup completion email is waiting for an authoritative Router client claim"
                .to_string(),
        ),
        SetupCompletionNotificationStatus::Pending => {
            Some(match notification.last_error.as_deref() {
                Some(error) => format!("setup completion email is pending retry: {error}"),
                None => "setup completion email is pending Router acknowledgement".to_string(),
            })
        }
        SetupCompletionNotificationStatus::TerminalFailed => {
            Some(match notification.last_error.as_deref() {
                Some(error) => format!("setup completion email permanently failed: {error}"),
                None => "setup completion email permanently failed".to_string(),
            })
        }
        SetupCompletionNotificationStatus::Acknowledged
            if notification.router_ack_status.as_deref() == Some("suppressed_disabled") =>
        {
            Some(
                "Router recorded setup completion, but Client email notifications are disabled; no email was queued"
                    .to_string(),
            )
        }
        SetupCompletionNotificationStatus::Acknowledged => None,
    }
}

fn now_ms_i64() -> i64 {
    crate::infra::time::now_ms().min(i64::MAX as u128) as i64
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
            start_client_tunnel: true,
            issue_session_token: false,
            issue_api_token: false,
            skip_router_claim: args.skip_router_claim,
        },
    )
    .await
    .map_err(|error| anyhow::anyhow!(error.message))?;

    if outcome.already_complete {
        println!("setup already complete in {}", state.config_dir.display());
        if let Some(status) = outcome.setup_completion_notification_status.as_deref() {
            println!("setup completion email status: {status}");
        }
        for warning in &outcome.warnings {
            println!("warning: {warning}");
        }
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
    if let Some(status) = outcome.setup_completion_notification_status.as_deref() {
        println!("setup completion email status: {status}");
    }
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
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::extract::State as AxumState;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use serde_json::{json, Value};
    use tokio::sync::Mutex as TokioMutex;

    use crate::cli::Cli;
    use crate::logging::{LogCapture, RING_BUFFER_CAPACITY};
    use crate::state::ServerStateInner;

    use super::*;

    fn test_state() -> ServerState {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let config_dir = std::env::temp_dir().join(format!("cc-switch-server-setup-test-{nanos}"));
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

    fn test_setup_input() -> SetupInput {
        SetupInput {
            password: "password123".to_string(),
            owner_email: "owner@example.com".to_string(),
            router_url: "http://127.0.0.1:9".to_string(),
            client_tunnel_subdomain: Some("ownerabcde".to_string()),
            options: Some(SetupOptions {
                allow_offline: true,
                ..SetupOptions::default()
            }),
        }
    }

    #[test]
    fn setup_web_url_uses_localhost_for_unspecified_bind() {
        let addr: SocketAddr = "0.0.0.0:15721".parse().unwrap();
        assert_eq!(setup_web_url(addr), "http://127.0.0.1:15721");
    }

    #[test]
    fn password_hint_is_fixed_width_ascii_without_length() {
        assert_eq!(password_hint("paraview"), "p******w");
        assert_eq!(password_hint("a-very-long-password9"), "a******9");
        assert_eq!(password_hint("!password?"), "********");
        assert_eq!(password_hint("密password码"), "********");
        for password in [
            "paraview",
            "a-very-long-password9",
            "!password?",
            "密password码",
        ] {
            let hint = password_hint(password);
            assert_eq!(hint.len(), 8);
            assert!(hint.is_ascii());
            assert_eq!(&hint[1..7], "******");
        }
    }

    #[test]
    fn generated_setup_id_is_lowercase_uuid_v4() {
        for _ in 0..16 {
            let setup_id = generate_setup_id();
            assert_eq!(setup_id.len(), 36);
            assert_eq!(setup_id.as_bytes()[14], b'4');
            assert!(matches!(setup_id.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
            assert!(setup_id.bytes().enumerate().all(|(index, byte)| matches!(
                index,
                8 | 13 | 18 | 23
            ) && byte == b'-'
                || !matches!(index, 8 | 13 | 18 | 23)
                    && (byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))));
        }
    }

    #[test]
    fn setup_outcome_json_exposes_status_without_hint() {
        let mut config = ServerConfig::empty();
        let mut notification = SetupCompletionNotificationState::new(
            "123e4567-e89b-42d3-a456-426614174000".to_string(),
            "p******w".to_string(),
            100,
        );
        notification.status = SetupCompletionNotificationStatus::Pending;
        config.setup_completion_notification = Some(notification);
        let outcome = SetupOutcome::from_provision(
            &config,
            "claimed",
            Vec::new(),
            false,
            "setup complete",
            None,
            None,
        );

        let json = serde_json::to_value(outcome).unwrap();
        assert_eq!(json["setupCompletionNotificationStatus"], "pending");
        assert_eq!(json["warnings"].as_array().unwrap().len(), 1);
        let encoded = serde_json::to_string(&json).unwrap();
        assert!(!encoded.contains("p******w"));
        assert!(!encoded.contains("passwordHint"));
    }

    #[test]
    fn suppressed_router_ack_warns_for_initial_and_idempotent_outcomes() {
        let mut config = ServerConfig::empty();
        let mut notification = SetupCompletionNotificationState::new(
            "123e4567-e89b-42d3-a456-426614174000".to_string(),
            "p******w".to_string(),
            100,
        );
        notification.status = SetupCompletionNotificationStatus::Acknowledged;
        notification.password_hint = None;
        notification.router_ack_status = Some("suppressed_disabled".to_string());
        config.setup_completion_notification = Some(notification);

        let initial = SetupOutcome::from_provision(
            &config,
            "claimed",
            Vec::new(),
            false,
            "setup complete",
            None,
            None,
        );
        let idempotent = SetupOutcome::already_complete(&config);

        for outcome in [initial, idempotent] {
            assert_eq!(
                outcome.setup_completion_notification_status.as_deref(),
                Some("acknowledged")
            );
            assert!(outcome.warnings.iter().any(|warning| {
                warning.contains("email notifications are disabled")
                    && warning.contains("no email was queued")
            }));
        }
    }

    #[tokio::test]
    async fn skip_router_claim_does_not_create_setup_completion_notification() {
        let state = test_state();
        let outcome = complete_setup(
            &state,
            test_setup_input(),
            SetupExecution {
                start_client_tunnel: false,
                skip_router_claim: true,
                ..SetupExecution::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(
            outcome.client_tunnel_claim_status.as_deref(),
            Some("skipped")
        );
        assert!(outcome.setup_completion_notification_status.is_none());
        assert!(state
            .config_snapshot()
            .await
            .setup_completion_notification
            .is_none());
    }

    #[tokio::test]
    async fn idempotent_already_complete_setup_does_not_create_notification() {
        let state = test_state();
        let mut config = ServerConfig::from_setup(test_setup_input()).unwrap();
        config.client.tunnel_status = Some("claim_skipped".to_string());
        state.replace_config(config).await.unwrap();

        let outcome = complete_setup(
            &state,
            test_setup_input(),
            SetupExecution {
                idempotent: true,
                start_client_tunnel: false,
                ..SetupExecution::default()
            },
        )
        .await
        .unwrap();

        assert!(outcome.already_complete);
        assert!(outcome.setup_completion_notification_status.is_none());
        assert!(state
            .config_snapshot()
            .await
            .setup_completion_notification
            .is_none());
    }

    #[tokio::test]
    async fn concurrent_setups_cannot_mix_password_hash_and_hint() {
        async fn available() -> Json<Value> {
            Json(json!({"available": true}))
        }
        async fn register() -> Json<Value> {
            Json(json!({
                "installationId": "installation-concurrent-setup",
                "controlSecret": "control-secret"
            }))
        }
        async fn bind_owner() -> Json<Value> {
            Json(json!({
                "ok": true,
                "ownerEmail": "owner@example.com",
                "ownerVerified": true,
                "alreadyBound": false
            }))
        }
        async fn owner() -> Json<Value> {
            Json(json!({
                "ok": true,
                "ownerEmail": "owner@example.com",
                "ownerVerified": true
            }))
        }
        async fn claimed() -> Json<Value> {
            Json(json!({"ok": true}))
        }
        async fn setup_completed(
            AxumState(hints): AxumState<Arc<TokioMutex<Vec<String>>>>,
            Json(request): Json<Value>,
        ) -> Json<Value> {
            hints.lock().await.push(
                request["setup"]["passwordHint"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            );
            Json(json!({
                "ok": true,
                "status": "suppressed_disabled",
                "setupId": request["setup"]["setupId"]
            }))
        }

        let hints = Arc::new(TokioMutex::new(Vec::new()));
        let router = Router::new()
            .route("/v1/client-tunnel/subdomain-availability", get(available))
            .route("/v1/installations/register", post(register))
            .route("/v1/installations/bind-owner-email", post(bind_owner))
            .route("/v1/installations/owner-email", get(owner))
            .route("/v1/installations/client-tunnel/claim", post(claimed))
            .route("/v1/installations/setup-completed", post(setup_completed))
            .with_state(hints.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let router_addr = listener.local_addr().unwrap();
        let router_server =
            tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let state = test_state();
        let input = |password: &str| SetupInput {
            password: password.to_string(),
            owner_email: "owner@example.com".to_string(),
            router_url: format!("http://{router_addr}"),
            client_tunnel_subdomain: Some("ownerabcde".to_string()),
            options: Some(SetupOptions {
                allow_offline: false,
                ..SetupOptions::default()
            }),
        };
        let exec = SetupExecution {
            start_client_tunnel: false,
            ..SetupExecution::default()
        };

        let (first, second) = tokio::join!(
            complete_setup(&state, input("alpha-password-1"), exec.clone()),
            complete_setup(&state, input("beta-password-2"), exec),
        );

        let (outcome, rejected) = match (first, second) {
            (Ok(outcome), Err(rejected)) | (Err(rejected), Ok(outcome)) => (outcome, rejected),
            results => panic!("expected exactly one setup success, got {results:?}"),
        };
        assert_eq!(rejected.status, StatusCode::CONFLICT);
        assert!(outcome.warnings.iter().any(|warning| {
            warning.contains("email notifications are disabled")
                && warning.contains("no email was queued")
        }));
        let config = state.config_snapshot().await;
        let expected_hint = if config.verify_password("alpha-password-1") {
            assert!(!config.verify_password("beta-password-2"));
            "a******1"
        } else {
            assert!(config.verify_password("beta-password-2"));
            "b******2"
        };
        assert_eq!(hints.lock().await.as_slice(), &[expected_hint.to_string()]);
        assert_eq!(
            config
                .setup_completion_notification
                .as_ref()
                .and_then(|notification| notification.router_ack_status.as_deref()),
            Some("suppressed_disabled")
        );
        router_server.abort();
    }
}
