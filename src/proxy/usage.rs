use crate::domain::providers::store::StoredProvider;
use crate::domain::usage::pricing::{calculate_cost, pricing_for_model_with_store};
use crate::domain::usage::store::{TokenUsage, UsageLog, UsageLogContext, UsageModelMetadata};
use crate::state::{ServerEvent, ServerState};

pub(super) async fn log_usage(
    state: &ServerState,
    stored: &StoredProvider,
    status_code: u16,
    duration_ms: u128,
    model: UsageModelMetadata,
    usage: TokenUsage,
    context: UsageLogContext,
) -> String {
    let usage_for_cost = usage;
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
    log.apply_context(context);
    let pricing_store = state.pricing.read().await.clone();
    if let Some(pricing) = pricing_for_model_with_store(
        &stored.provider,
        Some(&pricing_store),
        effective_pricing_model(&log),
    ) {
        log.apply_cost(calculate_cost(usage_for_cost, pricing));
    }
    let request_id = log.request_id.clone();
    if let Err(error) = state.push_usage_log(log).await {
        tracing::warn!("failed to persist usage log: {error}");
    }
    state.emit_event(
        ServerEvent::new("usage.created", "usage")
            .id(request_id.clone())
            .app(stored.app),
    );
    crate::state::sync_latest_direct_share_log(state.clone()).await;
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
    let pricing_store = state.pricing.read().await.clone();
    let persisted = state
        .update_usage_log(request_id, |log| {
            let router_visible_changed = apply_stream_usage_fields(
                log,
                status_code,
                duration_ms,
                first_token_ms,
                usage,
                stream_status,
            );
            if let Some(pricing) = pricing_for_model_with_store(
                &stored.provider,
                Some(&pricing_store),
                effective_pricing_model(log),
            ) {
                log.apply_cost(calculate_cost(log.token_usage(), pricing));
            }
            if router_visible_changed {
                log.router_last_synced_at_ms = None;
                log.router_last_sync_error = None;
                log.router_sync_attempt_count = 0;
            }
        })
        .await;
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
    crate::state::sync_latest_direct_share_log(state.clone()).await;
}

fn apply_stream_usage_fields(
    log: &mut UsageLog,
    status_code: u16,
    duration_ms: u128,
    first_token_ms: Option<u128>,
    usage: TokenUsage,
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
    if let Some(billed_input_tokens) = usage.billed_input_tokens {
        log.billed_input_tokens = Some(billed_input_tokens);
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
        log.total_tokens = Some(total_tokens);
    }
    if let Some(stream_status) = stream_status {
        log.stream_status = Some(stream_status.to_string());
    }
    router_visible_changed
}

fn effective_pricing_model(log: &UsageLog) -> Option<&str> {
    log.pricing_model
        .as_deref()
        .or(log.actual_model.as_deref())
        .or(log.requested_model.as_deref())
        .or(log.model.as_deref())
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
    use crate::domain::providers::model::{AppKind, Provider};
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
                        payloads.lock().await.push(body);
                        Json(json!({"ok": true}))
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
            .mutate_providers(|providers| {
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
            .await;
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
                        sale_market_kind: None,
                        access_by_app: BTreeMap::new(),
                        app_settings: BTreeMap::new(),
                        for_sale_official_price_percent_by_app: BTreeMap::new(),
                        official_price_percent: None,
                        auto_start: None,
                        description: None,
                        bindings: vec![ShareBinding {
                            app: AppKind::Codex,
                            provider_id: stored.provider.id.clone(),
                            provider_type: stored.provider_type,
                        }],
                        runtime_snapshot: None,
                        market_grant: None,
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
                pricing_model: Some("gpt-5.5".to_string()),
            },
            TokenUsage::default(),
            UsageLogContext {
                request_id: Some(REQUEST_ID.to_string()),
                share_id: Some("share-stream-usage".to_string()),
                share_name: Some("Stream Usage Share".to_string()),
                data_source: Some("direct".to_string()),
                is_streaming: true,
                ..UsageLogContext::default()
            },
        )
        .await;
        assert_eq!(logged_request_id, REQUEST_ID);

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

        let payloads = payloads.lock().await;
        assert_eq!(payloads.len(), 2);
        assert_eq!(payloads[0]["logs"][0]["requestId"], REQUEST_ID);
        assert_eq!(payloads[0]["logs"][0]["inputTokens"], 0);
        assert_eq!(payloads[0]["logs"][0]["latencyMs"], 5);
        assert_eq!(payloads[1]["logs"][0]["requestId"], REQUEST_ID);
        assert_eq!(payloads[1]["logs"][0]["statusCode"], 200);
        assert_eq!(payloads[1]["logs"][0]["latencyMs"], 321);
        assert_eq!(payloads[1]["logs"][0]["firstTokenMs"], 42);
        assert_eq!(payloads[1]["logs"][0]["inputTokens"], 11);
        assert_eq!(payloads[1]["logs"][0]["outputTokens"], 7);
        assert_eq!(payloads[1]["logs"][0]["cacheReadTokens"], 3);
        assert_eq!(payloads[1]["logs"][0]["cacheCreationTokens"], 2);
        drop(payloads);

        let usage = state.usage_snapshot().await;
        let log = usage
            .logs
            .iter()
            .find(|log| log.request_id == REQUEST_ID)
            .unwrap();
        assert_eq!(log.stream_status.as_deref(), Some("completed"));
        assert_eq!(log.router_sync_attempt_count, 1);
        assert!(log.router_last_synced_at_ms.is_some());
        assert!(log.router_last_sync_error.is_none());
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
