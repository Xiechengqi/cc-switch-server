use crate::domain::providers::store::StoredProvider;
use crate::domain::usage::store::{
    ImageUsageMetadata, TokenUsage, UsageLog, UsageLogContext, UsageModelMetadata, UsageOutcome,
    UsageState,
};
use crate::logging::{opaque_ref, AuditEvent, AuditRequestDetails};
use crate::state::{ServerEvent, ServerState};

use super::streaming::StreamUsageResult;

pub(super) async fn log_usage(
    state: &ServerState,
    stored: &StoredProvider,
    status_code: u16,
    duration_ms: u128,
    model: UsageModelMetadata,
    usage: TokenUsage,
    context: UsageLogContext,
) -> String {
    let mut log = UsageLog::new(
        stored.app,
        stored.provider.id.clone(),
        stored.provider.name.clone(),
        stored.provider_type,
        status_code,
        duration_ms,
        model,
        usage,
    );
    log.bundle_id = crate::domain::providers::bundle::bundle_id(&stored.provider)
        .unwrap_or(stored.provider.id.as_str())
        .to_string();
    log.family_id =
        crate::domain::providers::bundle::bundle_family_id(&stored.provider).map(str::to_string);
    log.supported_apps = crate::domain::providers::bundle::bundle_supported_apps(&stored.provider)
        .unwrap_or_else(|| vec![stored.app]);
    log.profile_id = stored
        .resource
        .profile_id
        .as_ref()
        .map(|profile_id| profile_id.as_str().to_string());
    let auth_binding = stored
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.auth_binding.as_ref());
    let account_id = auth_binding.and_then(|binding| binding.account_id.as_deref());
    log.account_ref = account_id.map(|account_id| opaque_ref("account", account_id));
    log.auth_identity_generation =
        auth_binding.and_then(|binding| binding.auth_identity_generation);
    if let Some(account_id) = account_id {
        log.account_display = state
            .find_account_by_id(account_id)
            .await
            .and_then(|account| account.email)
            .or_else(|| log.account_ref.clone());
    }
    log.apply_context(context);
    let request_id = log.request_id.clone();
    let account_ref = stored
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.auth_binding.as_ref())
        .and_then(|binding| binding.account_id.as_deref())
        .map(|account_id| opaque_ref("account", account_id));
    let audit_details = AuditRequestDetails {
        provider_type: Some(stored.provider_type.as_str().to_string()),
        provider_ref: Some(opaque_ref("provider", &stored.provider.id)),
        account_ref,
        requested_model: log.requested_model.clone(),
        actual_model: log.actual_model.clone().or_else(|| log.model.clone()),
        upstream_status: Some(status_code),
        streaming: Some(log.is_streaming),
        stream_status: log.stream_status.clone(),
        input_tokens: log.input_tokens,
        output_tokens: log.output_tokens,
        total_tokens: log.total_tokens,
        ..AuditRequestDetails::default()
    };
    state.enrich_audit_request(&request_id, audit_details.clone());
    if state.mark_audit_route_selected(&request_id) {
        let mut route_selected = AuditEvent::new("inference.route.selected");
        route_selected.request_id = Some(request_id.clone());
        route_selected.app = Some(stored.app.as_str().to_string());
        audit_details.apply_to(&mut route_selected);
        state.emit_audit_event_best_effort(route_selected);
        tracing::info!(
            target: "cc_switch_server::request_audit",
            event = "inference.route.selected",
            request_id = %request_id,
            app = stored.app.as_str(),
            provider_type = stored.provider_type.as_str(),
            provider_ref = %opaque_ref("provider", &stored.provider.id),
            requested_model = log.requested_model.as_deref().unwrap_or("-"),
            actual_model = log.actual_model.as_deref().or(log.model.as_deref()).unwrap_or("-"),
            "inference route selected"
        );
    }
    if let Err(error) = state.push_usage_log(log).await {
        tracing::warn!("failed to persist usage log: {error}");
    }
    state.emit_event(
        ServerEvent::new("usage.created", "usage")
            .id(request_id.clone())
            .app(stored.app),
    );
    crate::state::notify_router_share_log_sync(state);
    request_id
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn update_stream_usage(
    state: &ServerState,
    stored: &StoredProvider,
    request_id: &str,
    status_code: u16,
    duration_ms: u128,
    first_token_ms: Option<u128>,
    usage: TokenUsage,
    stream_status: Option<&str>,
) {
    update_stream_usage_with_parse_status(
        state,
        stored,
        request_id,
        status_code,
        duration_ms,
        first_token_ms,
        usage,
        false,
        stream_status,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn update_stream_usage_result(
    state: &ServerState,
    stored: &StoredProvider,
    request_id: &str,
    status_code: u16,
    duration_ms: u128,
    first_token_ms: Option<u128>,
    result: StreamUsageResult,
    stream_status: Option<&str>,
) {
    update_stream_usage_with_parse_status(
        state,
        stored,
        request_id,
        status_code,
        duration_ms,
        first_token_ms,
        result.usage,
        result.parse_error,
        stream_status,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn update_stream_usage_with_parse_status(
    state: &ServerState,
    stored: &StoredProvider,
    request_id: &str,
    status_code: u16,
    duration_ms: u128,
    first_token_ms: Option<u128>,
    usage: TokenUsage,
    usage_parse_error: bool,
    stream_status: Option<&str>,
) {
    let persisted = state
        .update_usage_log(request_id, |log| {
            let router_visible_changed = apply_stream_usage_fields(
                log,
                status_code,
                duration_ms,
                first_token_ms,
                usage,
                usage_parse_error,
                stream_status,
            );
            if router_visible_changed {
                log.reset_router_sync_state();
            }
        })
        .await;
    state.enrich_audit_request(
        request_id,
        AuditRequestDetails {
            upstream_status: Some(status_code),
            first_token_ms: first_token_ms.map(|value| u64::try_from(value).unwrap_or(u64::MAX)),
            streaming: Some(true),
            stream_status: stream_status.map(str::to_string),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            total_tokens: usage.total_tokens,
            ..AuditRequestDetails::default()
        },
    );
    match persisted {
        Ok(Some(_)) => {}
        Ok(None) => return,
        Err(error) => tracing::warn!("failed to persist stream usage update: {error}"),
    }
    state.emit_event(
        ServerEvent::new("usage.updated", "usage")
            .id(request_id.to_string())
            .app(stored.app)
            .message(stream_status.unwrap_or("stream")),
    );
    crate::state::notify_router_share_log_sync(state);
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn update_image_stream_usage(
    state: &ServerState,
    stored: &StoredProvider,
    request_id: &str,
    status_code: u16,
    duration_ms: u128,
    first_token_ms: Option<u128>,
    usage: TokenUsage,
    stream_status: &str,
    error_message: Option<&str>,
    image: Option<ImageUsageMetadata>,
) {
    let persisted = state
        .update_usage_log(request_id, |log| {
            let mut router_visible_changed = apply_stream_usage_fields(
                log,
                status_code,
                duration_ms,
                first_token_ms,
                usage,
                false,
                Some(stream_status),
            );
            let next_error = error_message.map(str::to_string);
            router_visible_changed |= log.error_message != next_error;
            log.error_message = next_error;
            if let Some(image) = image {
                router_visible_changed |= log.image_count != Some(image.count)
                    || log.image_bytes != Some(image.bytes)
                    || log.image_format != image.format
                    || log.image_width != image.width
                    || log.image_height != image.height
                    || log.image_size != image.size;
                log.image_count = Some(image.count);
                log.image_bytes = Some(image.bytes);
                log.image_format = image.format;
                log.image_width = image.width;
                log.image_height = image.height;
                log.image_size = image.size;
            }
            if router_visible_changed {
                log.reset_router_sync_state();
            }
        })
        .await;
    state.enrich_audit_request(
        request_id,
        AuditRequestDetails {
            upstream_status: Some(status_code),
            first_token_ms: first_token_ms.map(|value| u64::try_from(value).unwrap_or(u64::MAX)),
            streaming: Some(true),
            stream_status: Some(stream_status.to_string()),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            total_tokens: usage.total_tokens,
            ..AuditRequestDetails::default()
        },
    );
    match persisted {
        Ok(Some(_)) => {}
        Ok(None) => return,
        Err(error) => tracing::warn!("failed to persist image stream usage update: {error}"),
    }
    state.emit_event(
        ServerEvent::new("usage.updated", "usage")
            .id(request_id.to_string())
            .app(stored.app)
            .message(stream_status),
    );
    crate::state::notify_router_share_log_sync(state);
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn update_websocket_stream_usage(
    state: &ServerState,
    stored: &StoredProvider,
    request_id: &str,
    status_code: u16,
    duration_ms: u128,
    first_token_ms: Option<u128>,
    usage: TokenUsage,
    stream_status: &str,
    error_message: Option<&str>,
) {
    let persisted = state
        .update_usage_log(request_id, |log| {
            let mut router_visible_changed = apply_stream_usage_fields(
                log,
                status_code,
                duration_ms,
                first_token_ms,
                usage,
                false,
                Some(stream_status),
            );
            let next_error = error_message.map(str::to_string);
            router_visible_changed |= log.error_message != next_error;
            log.error_message = next_error;
            if router_visible_changed {
                log.reset_router_sync_state();
            }
        })
        .await;
    state.enrich_audit_request(
        request_id,
        AuditRequestDetails {
            upstream_status: Some(status_code),
            first_token_ms: first_token_ms.map(|value| u64::try_from(value).unwrap_or(u64::MAX)),
            streaming: Some(true),
            stream_status: Some(stream_status.to_string()),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            total_tokens: usage.total_tokens,
            ..AuditRequestDetails::default()
        },
    );
    match persisted {
        Ok(Some(_)) => {}
        Ok(None) => return,
        Err(error) => tracing::warn!("failed to persist websocket stream usage update: {error}"),
    }
    state.emit_event(
        ServerEvent::new("usage.updated", "usage")
            .id(request_id.to_string())
            .app(stored.app)
            .message(stream_status),
    );
    crate::state::notify_router_share_log_sync(state);
}

fn apply_stream_usage_fields(
    log: &mut UsageLog,
    status_code: u16,
    duration_ms: u128,
    first_token_ms: Option<u128>,
    usage: TokenUsage,
    usage_parse_error: bool,
    stream_status: Option<&str>,
) -> bool {
    let mut router_visible_changed =
        log.status_code != status_code || log.duration_ms != duration_ms;
    log.status_code = status_code;
    log.duration_ms = duration_ms;

    if let Some(first_token_ms) = first_token_ms.filter(|_| log.first_token_ms.is_none()) {
        router_visible_changed = true;
        log.first_token_ms = Some(first_token_ms);
    }
    if let Some(input_tokens) = usage.input_tokens {
        router_visible_changed |= log.input_tokens != Some(input_tokens);
        log.input_tokens = Some(input_tokens);
    }
    if let Some(raw_input_tokens) = usage.raw_input_tokens {
        log.raw_input_tokens = Some(raw_input_tokens);
    }
    if let Some(output_tokens) = usage.output_tokens {
        router_visible_changed |= log.output_tokens != Some(output_tokens);
        log.output_tokens = Some(output_tokens);
    }
    if let Some(cache_read_tokens) = usage.cache_read_tokens {
        router_visible_changed |= log.cache_read_tokens != Some(cache_read_tokens);
        log.cache_read_tokens = Some(cache_read_tokens);
    }
    if let Some(cache_creation_tokens) = usage.cache_creation_tokens {
        router_visible_changed |= log.cache_creation_tokens != Some(cache_creation_tokens);
        log.cache_creation_tokens = Some(cache_creation_tokens);
    }
    if let Some(total_tokens) = usage.total_tokens {
        router_visible_changed |= log.total_tokens != Some(total_tokens);
        log.total_tokens = Some(total_tokens);
    }
    let next_stream_status = stream_status
        .map(str::to_string)
        .or_else(|| log.stream_status.clone());
    router_visible_changed |= log.stream_status != next_stream_status;
    log.stream_status = next_stream_status;

    let next_usage_state = usage_state_for_stream(usage, usage_parse_error, stream_status);
    router_visible_changed |= log.usage_state != next_usage_state;
    log.usage_state = next_usage_state;
    log.upstream_duration_ms = duration_ms;
    if next_usage_state == UsageState::Pending {
        log.completed_at_ms = 0;
        log.end_to_end_duration_ms = 0;
        log.outcome = UsageOutcome::Pending;
    } else {
        log.completed_at_ms = crate::infra::time::now_ms();
        log.end_to_end_duration_ms = log.completed_at_ms.saturating_sub(log.started_at_ms);
        log.outcome = if next_usage_state == UsageState::Interrupted {
            UsageOutcome::Interrupted
        } else {
            UsageOutcome::from_status(status_code)
        };
        log.failure_kind = if log.outcome == UsageOutcome::Success {
            None
        } else {
            stream_status.map(str::to_string)
        };
    }
    if router_visible_changed {
        log.usage_revision = log.usage_revision.saturating_add(1);
    }
    router_visible_changed
}

fn usage_state_for_stream(
    usage: TokenUsage,
    parse_error: bool,
    stream_status: Option<&str>,
) -> UsageState {
    if parse_error {
        return UsageState::ParseError;
    }
    if matches!(stream_status, Some("pending" | "streaming")) {
        return UsageState::Pending;
    }
    if matches!(
        stream_status,
        Some("client_cancelled" | "interrupted" | "timeout")
    ) {
        return UsageState::Interrupted;
    }
    if usage.has_observation() {
        UsageState::Observed
    } else {
        UsageState::Missing
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::routing::post;
    use axum::{Json, Router};
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use base64::Engine;
    use serde_json::{json, Value};
    use tokio::sync::Mutex;

    use super::*;
    use crate::cli::Cli;
    use crate::domain::providers::model::{AppKind, Provider, ProviderType};
    use crate::domain::settings::config::RouterIdentity;
    use crate::domain::sharing::shares::{ShareBinding, UpsertShareInput};
    use crate::logging::{LogCapture, RING_BUFFER_CAPACITY};
    use crate::state::ServerStateInner;

    #[tokio::test]
    async fn terminal_stream_update_resyncs_router_log_with_final_usage() {
        const REQUEST_ID: &str = "550e8400-e29b-41d4-a716-446655440001";

        let payloads = Arc::new(Mutex::new(Vec::<Value>::new()));
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let router_addr = listener.local_addr().unwrap();
        let router = Router::new()
            .route(
                "/v1/share-request-logs/batch-sync",
                post(
                    |axum::extract::State(payloads): axum::extract::State<
                        Arc<Mutex<Vec<Value>>>,
                    >,
                     Json(body): Json<Value>| async move {
                        let acks = body["logs"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|log| {
                                json!({
                                    "requestId": log["requestId"],
                                    "usageRevision": log["usageRevision"],
                                })
                            })
                            .collect::<Vec<_>>();
                        payloads.lock().await.push(body);
                        Json(json!({"ok": true, "acks": acks}))
                    },
                ),
            )
            .with_state(payloads.clone());
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });

        let state = test_state();
        let mut config = state.config_snapshot().await;
        config.router.url = Some(format!("http://{router_addr}"));
        config.router.identity = Some(RouterIdentity {
            installation_id: "inst-stream-usage".to_string(),
            public_key: BASE64_STANDARD.encode([8_u8; 32]),
            private_key: BASE64_STANDARD.encode([7_u8; 32]),
            control_secret: Some("control-secret".to_string()),
        });
        state.replace_config(config).await.unwrap();

        let stored = state
            .mutate_providers_immediate(|providers| {
                providers.upsert(
                    AppKind::Codex,
                    Provider {
                        id: "provider-stream-usage".to_string(),
                        name: "Provider Stream Usage".to_string(),
                        settings_config: json!({}),
                        category: None,
                        meta: None,
                        extra: BTreeMap::new(),
                    },
                )
            })
            .await
            .unwrap();
        state
            .mutate_shares_immediate(|shares| {
                shares
                    .upsert(UpsertShareInput {
                        id: Some("share-stream-usage".to_string()),
                        owner_email: Some("owner@example.com".to_string()),
                        app: AppKind::Codex,
                        provider_id: stored.provider.id.clone(),
                        provider_type: stored.provider_type,
                        display_name: Some("Stream Usage Share".to_string()),
                        enabled: None,
                        status: None,
                        subscription_level: None,
                        account_email: None,
                        quota_percent: None,
                        tunnel_subdomain: None,
                        acl: None,
                        token_limit: None,
                        parallel_limit: None,
                        expires_at: None,
                        for_sale: None,
                        free_access: None,
                        access_by_app: BTreeMap::new(),
                        app_settings: BTreeMap::new(),
                        for_sale_official_price_percent_by_app: BTreeMap::new(),
                        official_price_percent: None,
                        allow_personal_credits: None,
                        auto_consume_banked_reset: None,
                        banked_reset_expiry_lead_minutes: None,
                        previous_response_cache_enabled: None,
                        auto_start: None,
                        description: None,
                        enabled_apps: None,
                        bindings: vec![ShareBinding {
                            app: AppKind::Codex,
                            provider_id: stored.provider.id.clone(),
                            provider_type: stored.provider_type,
                        }],
                        runtime_snapshot: None,
                        user_grants: BTreeMap::new(),
                    })
                    .unwrap()
            })
            .await
            .unwrap();

        let logged_request_id = log_usage(
            &state,
            &stored,
            200,
            5,
            UsageModelMetadata {
                model: Some("gpt-5.5".to_string()),
                requested_model: Some("gpt-5.5".to_string()),
                actual_model: Some("gpt-5.5".to_string()),
                actual_model_source: Some("response".to_string()),
            },
            TokenUsage::default(),
            UsageLogContext {
                request_id: Some(REQUEST_ID.to_string()),
                share_id: Some("share-stream-usage".to_string()),
                share_name: Some("Stream Usage Share".to_string()),
                data_source: Some("router_share".to_string()),
                is_streaming: true,
                stream_status: Some("pending".to_string()),
                ..UsageLogContext::default()
            },
        )
        .await;
        assert_eq!(logged_request_id, REQUEST_ID);
        let initial_sync =
            crate::state::sync_pending_router_share_logs(state.clone(), 100, true).await;
        assert_eq!(initial_sync.synced, 1);

        update_stream_usage(
            &state,
            &stored,
            REQUEST_ID,
            200,
            321,
            Some(42),
            TokenUsage {
                input_tokens: Some(11),
                output_tokens: Some(7),
                cache_read_tokens: Some(3),
                cache_creation_tokens: Some(2),
                total_tokens: Some(23),
                ..TokenUsage::default()
            },
            Some("completed"),
        )
        .await;
        let terminal_sync =
            crate::state::sync_pending_router_share_logs(state.clone(), 100, true).await;
        assert_eq!(terminal_sync.synced, 1);

        let payloads = payloads.lock().await;
        assert_eq!(payloads.len(), 2);
        assert_eq!(payloads[0]["logs"][0]["requestId"], REQUEST_ID);
        assert_eq!(payloads[0]["logs"][0]["inputTokens"], 0);
        assert_eq!(payloads[0]["logs"][0]["latencyMs"], 5);
        assert_eq!(payloads[0]["logs"][0]["usageState"], "pending");
        assert_eq!(payloads[0]["logs"][0]["streamStatus"], "pending");
        assert_eq!(payloads[0]["logs"][0]["usageRevision"], 1);
        assert_eq!(payloads[1]["logs"][0]["requestId"], REQUEST_ID);
        assert_eq!(payloads[1]["logs"][0]["statusCode"], 200);
        assert_eq!(payloads[1]["logs"][0]["latencyMs"], 321);
        assert_eq!(payloads[1]["logs"][0]["firstTokenMs"], 42);
        assert_eq!(payloads[1]["logs"][0]["inputTokens"], 11);
        assert_eq!(payloads[1]["logs"][0]["outputTokens"], 7);
        assert_eq!(payloads[1]["logs"][0]["cacheReadTokens"], 3);
        assert_eq!(payloads[1]["logs"][0]["cacheCreationTokens"], 2);
        assert_eq!(payloads[1]["logs"][0]["usageState"], "observed");
        assert_eq!(payloads[1]["logs"][0]["streamStatus"], "completed");
        assert_eq!(payloads[1]["logs"][0]["usageRevision"], 2);
        drop(payloads);

        let usage = state.usage_snapshot().await;
        let log = usage
            .logs
            .iter()
            .find(|log| log.request_id == REQUEST_ID)
            .unwrap();
        assert_eq!(log.stream_status.as_deref(), Some("completed"));
        assert_eq!(log.usage_state, UsageState::Observed);
        assert_eq!(log.usage_revision, 2);
        assert_eq!(log.router_sync_attempt_count, 1);
        assert!(log.router_last_synced_at_ms.is_some());
        assert!(log.router_last_sync_error.is_none());
    }

    fn pending_usage_log() -> UsageLog {
        let mut log = UsageLog::new(
            AppKind::Codex,
            "provider".to_string(),
            "Provider".to_string(),
            ProviderType::CodexOAuth,
            200,
            1,
            UsageModelMetadata {
                model: Some("gpt-5.4".to_string()),
                ..UsageModelMetadata::default()
            },
            TokenUsage::default(),
        );
        log.apply_context(UsageLogContext {
            is_streaming: true,
            stream_status: Some("pending".to_string()),
            ..UsageLogContext::default()
        });
        log
    }

    #[test]
    fn stream_usage_state_distinguishes_explicit_zero_from_missing() {
        let mut log = pending_usage_log();
        assert_eq!(log.usage_state, UsageState::Pending);
        assert_eq!(log.usage_revision, 1);

        assert!(apply_stream_usage_fields(
            &mut log,
            200,
            10,
            Some(2),
            TokenUsage {
                input_tokens: Some(0),
                output_tokens: Some(0),
                ..TokenUsage::default()
            },
            false,
            Some("completed"),
        ));
        assert_eq!(log.usage_state, UsageState::Observed);
        assert_eq!(log.input_tokens, Some(0));
        assert_eq!(log.output_tokens, Some(0));
        assert_eq!(log.usage_revision, 2);

        let mut missing = pending_usage_log();
        apply_stream_usage_fields(
            &mut missing,
            200,
            10,
            None,
            TokenUsage::default(),
            false,
            Some("completed"),
        );
        assert_eq!(missing.usage_state, UsageState::Missing);
    }

    #[test]
    fn stream_usage_state_reports_parse_error_and_interruption() {
        let mut parse_error = pending_usage_log();
        apply_stream_usage_fields(
            &mut parse_error,
            200,
            10,
            None,
            TokenUsage::default(),
            true,
            Some("completed"),
        );
        assert_eq!(parse_error.usage_state, UsageState::ParseError);
        assert_eq!(parse_error.usage_revision, 2);

        let mut interrupted = pending_usage_log();
        apply_stream_usage_fields(
            &mut interrupted,
            200,
            10,
            None,
            TokenUsage {
                output_tokens: Some(4),
                ..TokenUsage::default()
            },
            false,
            Some("client_cancelled"),
        );
        assert_eq!(interrupted.usage_state, UsageState::Interrupted);
        assert_eq!(interrupted.output_tokens, Some(4));
        assert_eq!(interrupted.usage_revision, 2);
    }

    fn test_state() -> crate::state::ServerState {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let config_dir =
            std::env::temp_dir().join(format!("cc-switch-server-stream-usage-test-{nanos}"));
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
}
