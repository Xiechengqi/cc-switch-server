use crate::core::pricing::{calculate_cost, pricing_for_model_with_store};
use crate::core::providers::StoredProvider;
use crate::core::usage::{TokenUsage, UsageLog, UsageLogContext, UsageModelMetadata};
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
    if let Err(error) = state
        .usage
        .write()
        .await
        .push_and_persist(&state.config_dir, log)
    {
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
    let persisted =
        state
            .usage
            .write()
            .await
            .update_log_and_persist(&state.config_dir, request_id, |log| {
                log.status_code = status_code;
                log.duration_ms = duration_ms;
                if first_token_ms.is_some() && log.first_token_ms.is_none() {
                    log.first_token_ms = first_token_ms;
                }
                if usage.input_tokens.is_some() {
                    log.input_tokens = usage.input_tokens;
                }
                if usage.raw_input_tokens.is_some() {
                    log.raw_input_tokens = usage.raw_input_tokens;
                }
                if usage.billed_input_tokens.is_some() {
                    log.billed_input_tokens = usage.billed_input_tokens;
                }
                if usage.output_tokens.is_some() {
                    log.output_tokens = usage.output_tokens;
                }
                if usage.cache_read_tokens.is_some() {
                    log.cache_read_tokens = usage.cache_read_tokens;
                }
                if usage.cache_creation_tokens.is_some() {
                    log.cache_creation_tokens = usage.cache_creation_tokens;
                }
                if usage.total_tokens.is_some() {
                    log.total_tokens = usage.total_tokens;
                }
                if let Some(pricing) = pricing_for_model_with_store(
                    &stored.provider,
                    Some(&pricing_store),
                    effective_pricing_model(log),
                ) {
                    log.apply_cost(calculate_cost(log.token_usage(), pricing));
                }
                if let Some(stream_status) = stream_status {
                    log.stream_status = Some(stream_status.to_string());
                }
            });
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

fn effective_pricing_model(log: &UsageLog) -> Option<&str> {
    log.pricing_model
        .as_deref()
        .or(log.actual_model.as_deref())
        .or(log.requested_model.as_deref())
        .or(log.model.as_deref())
}
