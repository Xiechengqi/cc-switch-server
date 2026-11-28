//! Buffered Zen decoder lifecycle. The forwarder supplies explicit request,
//! timing, usage and memory owners; this module owns neither state writes nor
//! retry/settlement policy. Returned reservation follows the resulting Value.
use crate::{
    domain::providers::store::StoredProvider,
    proxy::{
        provider_ops::ProviderExecution,
        request_memory::{RequestMemoryBudget, RequestMemoryComponent, RequestMemoryReservation},
        request_timing::ForwardTiming,
        streaming::StreamUsageAccumulator,
        ProxyError, ProxyRoute,
    },
};
use futures_util::StreamExt;
use http::{header::CONTENT_TYPE, StatusCode};
use serde_json::Value;
use std::time::{Duration, Instant};

#[allow(clippy::too_many_arguments)]
pub(super) async fn read_document(
    response: crate::proxy::opencode_zen::transport::ZenResponse,
    stored: &StoredProvider,
    route: ProxyRoute,
    prepared: &crate::proxy::opencode_zen::PreparedUpstream,
    budget: &RequestMemoryBudget,
    execution: &ProviderExecution,
    started: Instant,
    deadline: tokio::time::Instant,
    timing: Option<ForwardTiming>,
    usage: &mut StreamUsageAccumulator,
    first_token_ms: &mut Option<u128>,
) -> Result<(Value, RequestMemoryReservation), ProxyError> {
    use crate::proxy::opencode_zen as zen;
    if !response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"))
    {
        return Err(ProxyError::bad_gateway(
            "OpenCode Zen expected an SSE response",
        ));
    }
    let mut decoder = zen::ZenStream::new(
        stored,
        route,
        prepared.wire,
        Default::default(),
        Default::default(),
        true,
        timing,
    )
    .with_client_tools(prepared.client_tool_names.clone())
    .with_codex_exec(prepared.codex_exec_tool.clone())
    .with_aggregation();
    let retained = budget
        .reserve(
            RequestMemoryComponent::StreamRetainedState,
            decoder.retained_bytes(),
        )
        .map_err(|e| e.into_proxy_error())?;
    let first_deadline = tokio::time::Instant::from_std(started)
        + execution
            .stream_first_byte_timeout()
            .unwrap_or(Duration::from_secs(60));
    let idle = execution
        .stream_idle_timeout()
        .unwrap_or(Duration::from_secs(60));
    let mut idle_deadline = tokio::time::Instant::now() + idle;
    let mut stream = response.bytes_stream();
    loop {
        let read_deadline = if decoder.semantic {
            deadline
        } else {
            deadline.min(first_deadline)
        };
        let chunk = tokio::time::timeout_at(read_deadline.min(idle_deadline), stream.next())
            .await
            .map_err(|_| ProxyError {
                status: StatusCode::GATEWAY_TIMEOUT,
                message: "OpenCode Zen stream deadline exceeded".into(),
            })?;
        let Some(chunk) = chunk else {
            break;
        };
        let chunk = chunk?;
        idle_deadline = tokio::time::Instant::now() + idle;
        let _pending = budget
            .reserve(RequestMemoryComponent::TransportPending, chunk.len())
            .map_err(|e| e.into_proxy_error())?;
        retained
            .resize(
                decoder
                    .retained_bytes()
                    .saturating_add(usage.retained_bytes())
                    .saturating_add(decoder.working_bytes(&chunk)),
            )
            .map_err(|e| e.into_proxy_error())?;
        usage.push(&chunk);
        decoder.push(&chunk)?;
        if decoder.semantic && first_token_ms.is_none() {
            *first_token_ms = Some(started.elapsed().as_millis());
        }
        retained
            .resize(
                decoder
                    .retained_bytes()
                    .saturating_add(usage.retained_bytes()),
            )
            .map_err(|e| e.into_proxy_error())?;
    }
    let payload = decoder.finish_document()?;
    let argument_working = payload
        .pointer("/choices/0/message/tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|call| call.pointer("/function/arguments").and_then(Value::as_str))
        .map(|args| crate::proxy::request_memory::json_parse_working_bytes(args.as_bytes()))
        .sum::<usize>();
    retained
        .resize(
            crate::proxy::request_memory::retained_json_bytes(&payload)
                .saturating_mul(4)
                .saturating_add(argument_working),
        )
        .map_err(|e| e.into_proxy_error())?;
    Ok((payload, retained))
}
