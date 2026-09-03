use std::sync::OnceLock;

use anyhow::Context;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

use crate::domain::health::ProviderRequestOutcome;
use crate::domain::sharing::previous_response_cache::PreviousResponseCacheStats;

static PROMETHEUS_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

pub fn init() -> anyhow::Result<()> {
    if PROMETHEUS_HANDLE.get().is_some() {
        return Ok(());
    }
    let handle = PrometheusBuilder::new()
        .install_recorder()
        .context("install Prometheus metrics recorder")?;
    let _ = PROMETHEUS_HANDLE.set(handle);
    describe();
    set_credential_persistence_degraded(false);
    set_claude_wire_profile_info();
    Ok(())
}

pub fn render() -> String {
    PROMETHEUS_HANDLE
        .get()
        .map(PrometheusHandle::render)
        .unwrap_or_default()
}

pub fn record_account_inflight(provider_type: &str, current: u32) {
    metrics::gauge!(
        "cc_switch_account_inflight",
        "provider_type" => provider_type.to_string()
    )
    .set(f64::from(current));
}

pub fn record_claude_retry(stage: &str, source: &str) {
    metrics::counter!(
        "cc_switch_claude_retry_total",
        "stage" => stage.to_string(),
        "source" => source.to_string()
    )
    .increment(1);
}

pub fn record_forward_retry(app: &str, stage: &str, source: &str) {
    metrics::counter!(
        "cc_switch_forward_retry_total",
        "app" => app.to_string(),
        "stage" => stage.to_string(),
        "source" => source.to_string()
    )
    .increment(1);
}

pub fn record_codex_websocket_cache(result: &'static str) {
    metrics::counter!(
        "cc_switch_codex_websocket_cache_total",
        "result" => result
    )
    .increment(1);
}

pub fn record_codex_websocket_fallback(source: &'static str, result: &'static str) {
    metrics::counter!(
        "cc_switch_codex_websocket_fallback_total",
        "source" => source,
        "result" => result
    )
    .increment(1);
}

pub fn record_codex_responses_lite_decision(decision: &'static str) {
    metrics::counter!(
        "cc_switch_codex_responses_lite_total",
        "decision" => decision
    )
    .increment(1);
}

pub fn record_codex_metadata_decision(decision: &'static str) {
    metrics::counter!(
        "cc_switch_codex_metadata_total",
        "decision" => decision
    )
    .increment(1);
}

pub fn record_codex_routing_hint(decision: &'static str) {
    metrics::counter!(
        "cc_switch_codex_routing_hint_total",
        "decision" => decision
    )
    .increment(1);
}

pub fn record_responses_downstream_keepalive(surface: &'static str) {
    metrics::counter!(
        "cc_switch_responses_downstream_keepalives_total",
        "surface" => surface
    )
    .increment(1);
}

pub fn record_responses_sse_transport(surface: &'static str, observation: &'static str) {
    metrics::counter!(
        "cc_switch_responses_sse_transport_total",
        "surface" => surface,
        "observation" => observation
    )
    .increment(1);
}

pub fn set_previous_response_cache_stats(stats: &PreviousResponseCacheStats) {
    for (name, value) in [
        ("current_entries", stats.current_entries as f64),
        ("current_bytes", stats.current_bytes as f64),
        ("current_tombstones", stats.current_tombstones as f64),
        ("high_water_entries", stats.high_water_entries as f64),
        ("high_water_bytes", stats.high_water_bytes as f64),
        (
            "max_observed_entry_bytes",
            stats.max_observed_entry_bytes as f64,
        ),
        (
            "max_observed_entry_items",
            stats.max_observed_entry_items as f64,
        ),
        ("hits_total", stats.hits as f64),
        ("misses_total", stats.misses as f64),
        ("expired_total", stats.expired as f64),
        ("count_evictions_total", stats.count_evictions as f64),
        ("byte_evictions_total", stats.byte_evictions as f64),
        (
            "oversize_entry_rejections_total",
            stats.oversize_entry_rejections as f64,
        ),
        (
            "too_many_items_rejections_total",
            stats.too_many_items_rejections as f64,
        ),
        (
            "invalid_response_id_rejections_total",
            stats.invalid_response_id_rejections as f64,
        ),
        (
            "required_context_unavailable_total",
            stats.required_context_unavailable as f64,
        ),
    ] {
        metrics::gauge!(
            "cc_switch_previous_response_cache",
            "stat" => name
        )
        .set(value);
    }
}

pub fn record_codex_images_request(
    operation: &'static str,
    streaming: bool,
    outcome: &'static str,
) {
    metrics::counter!(
        "cc_switch_codex_images_requests_total",
        "operation" => operation,
        "mode" => if streaming { "stream" } else { "json" },
        "outcome" => outcome
    )
    .increment(1);
}

pub fn record_codex_images_output(operation: &'static str, format: &str, count: u64, bytes: u64) {
    metrics::counter!(
        "cc_switch_codex_images_output_total",
        "operation" => operation,
        "format" => format.to_string()
    )
    .increment(count);
    metrics::counter!(
        "cc_switch_codex_images_output_bytes_total",
        "operation" => operation,
        "format" => format.to_string()
    )
    .increment(bytes);
}

pub fn record_image_capability_event(event: &'static str) {
    metrics::counter!(
        "cc_switch_image_capability_events_total",
        "event" => event
    )
    .increment(1);
}

pub fn set_image_capability_store_size(entries: usize, bytes: u64) {
    metrics::gauge!("cc_switch_image_capability_entries").set(entries as f64);
    metrics::gauge!("cc_switch_image_capability_bytes").set(bytes as f64);
}

pub fn record_image_transport_first_byte(
    surface: &'static str,
    mode: &'static str,
    elapsed: std::time::Duration,
) {
    metrics::histogram!(
        "cc_switch_image_transport_first_byte_seconds",
        "surface" => surface,
        "mode" => mode
    )
    .record(elapsed.as_secs_f64());
}

pub fn record_image_transport_heartbeat(surface: &'static str, mode: &'static str) {
    metrics::counter!(
        "cc_switch_image_transport_heartbeats_total",
        "surface" => surface,
        "mode" => mode
    )
    .increment(1);
}

pub fn record_image_transport_max_silence(
    surface: &'static str,
    mode: &'static str,
    silence: std::time::Duration,
) {
    metrics::histogram!(
        "cc_switch_image_transport_max_silence_seconds",
        "surface" => surface,
        "mode" => mode
    )
    .record(silence.as_secs_f64());
}

pub fn record_provider_outcome(app: &str, provider_type: &str, outcome: ProviderRequestOutcome) {
    let outcome = match outcome {
        ProviderRequestOutcome::Success { .. } => "success",
        ProviderRequestOutcome::Failure { .. } => "failure",
        ProviderRequestOutcome::RateLimited { .. } => "rate_limited",
        ProviderRequestOutcome::CapacityShed { .. } => "capacity_shed",
        ProviderRequestOutcome::NetworkFailure => "network_failure",
    };
    metrics::counter!(
        "cc_switch_provider_outcome_total",
        "app" => app.to_string(),
        "provider_type" => provider_type.to_string(),
        "outcome" => outcome
    )
    .increment(1);
}

pub fn record_warm_refresh(provider_type: &str, result: &str) {
    metrics::counter!(
        "cc_switch_account_warm_refresh_total",
        "provider_type" => provider_type.to_string(),
        "result" => result.to_string()
    )
    .increment(1);
}

pub fn record_oauth_refresh_attempt(
    provider_type: &'static str,
    outcome: &'static str,
    elapsed: std::time::Duration,
    outcome_unknown: bool,
) {
    metrics::counter!(
        "cc_switch_oauth_refresh_attempt_total",
        "provider_type" => provider_type,
        "outcome" => outcome
    )
    .increment(1);
    metrics::histogram!(
        "cc_switch_oauth_refresh_attempt_duration_seconds",
        "provider_type" => provider_type,
        "outcome" => outcome
    )
    .record(elapsed.as_secs_f64());
    if outcome_unknown {
        metrics::counter!(
            "cc_switch_oauth_refresh_unknown_outcome_total",
            "provider_type" => provider_type
        )
        .increment(1);
    }
}

pub fn record_account_lease(provider_type: &'static str, result: &'static str) {
    metrics::counter!(
        "cc_switch_account_lease_total",
        "provider_type" => provider_type,
        "result" => result
    )
    .increment(1);
}

pub fn set_credential_persistence_degraded(degraded: bool) {
    metrics::gauge!("cc_switch_credential_persistence_degraded").set(if degraded {
        1.0
    } else {
        0.0
    });
}

pub fn record_claude_cli_version_gate() {
    metrics::counter!("cc_switch_claude_cli_version_gate_total").increment(1);
}

pub fn record_grok_cli_version_gate() {
    metrics::counter!("cc_switch_grok_cli_version_gate_total").increment(1);
}

pub fn record_grok_model_catalog(source: &'static str) {
    metrics::counter!(
        "cc_switch_grok_model_catalog_total",
        "source" => source
    )
    .increment(1);
}

pub fn record_qoder_client_ip_source(source: &'static str) {
    metrics::counter!(
        "cc_switch_qoder_client_ip_total",
        "source" => source
    )
    .increment(1);
}

pub fn record_qoder_compatibility(kind: &'static str) {
    metrics::counter!(
        "cc_switch_qoder_compatibility_total",
        "kind" => kind
    )
    .increment(1);
}

pub fn record_qoder_error(kind: &'static str) {
    metrics::counter!(
        "cc_switch_qoder_error_total",
        "kind" => kind
    )
    .increment(1);
}

pub fn record_claude_bootstrap(result: &str) {
    metrics::counter!(
        "cc_switch_claude_bootstrap_total",
        "result" => result.to_string()
    )
    .increment(1);
}

pub fn record_claude_roles(result: &'static str) {
    metrics::counter!(
        "cc_switch_claude_roles_total",
        "result" => result
    )
    .increment(1);
}

pub fn record_claude_beta_decision(operation: &'static str, decision: &'static str) {
    metrics::counter!(
        "cc_switch_claude_beta_decision_total",
        "operation" => operation,
        "decision" => decision
    )
    .increment(1);
}

pub fn record_claude_client_class(class: &'static str, operation: &'static str) {
    metrics::counter!(
        "cc_switch_claude_client_class_total",
        "class" => class,
        "operation" => operation
    )
    .increment(1);
}

pub fn record_claude_rate_limit_scope(scope: &'static str) {
    metrics::counter!("cc_switch_claude_rate_limit_scope_total", "scope" => scope).increment(1);
}

pub fn record_claude_quota_header_observation(outcome: &'static str) {
    metrics::counter!(
        "cc_switch_claude_quota_header_observation_total",
        "outcome" => outcome
    )
    .increment(1);
}

pub fn record_claude_optional_rewrite(kind: &'static str) {
    metrics::counter!("cc_switch_claude_optional_rewrite_total", "kind" => kind).increment(1);
}

pub fn record_claude_ttfb(elapsed: std::time::Duration) {
    metrics::histogram!("cc_switch_claude_ttfb_seconds").record(elapsed.as_secs_f64());
}

pub fn record_claude_stream_duration(outcome: &'static str, elapsed: std::time::Duration) {
    metrics::histogram!(
        "cc_switch_claude_stream_duration_seconds",
        "outcome" => outcome
    )
    .record(elapsed.as_secs_f64());
}

pub fn record_claude_semantic_failure(stage: &'static str, kind: &'static str) {
    metrics::counter!(
        "cc_switch_claude_semantic_failure_total",
        "stage" => stage,
        "kind" => kind
    )
    .increment(1);
}

pub fn record_claude_response_decoding(surface: &'static str, result: &'static str) {
    metrics::counter!(
        "cc_switch_claude_response_decoding_total",
        "surface" => surface,
        "result" => result
    )
    .increment(1);
}

fn set_claude_wire_profile_info() {
    let profile = crate::domain::claude_cli::CLAUDE_WIRE_PROFILE;
    let identity = crate::domain::claude_cli::claude_cli_identity();
    let stale_override_rejected = if identity.stale_override_rejected {
        "true"
    } else {
        "false"
    };
    metrics::gauge!(
        "cc_switch_claude_wire_profile_info",
        "profile_id" => profile.id,
        "claude_code_version" => profile.claude_code_version,
        "effective_claude_code_version" => identity.version,
        "identity_source" => identity.source,
        "stale_override_rejected" => stale_override_rejected,
        "stainless_version" => profile.stainless_package_version,
        "node_version" => profile.node_version,
        "axios_version" => profile.axios_version
    )
    .set(1.0);
}

pub fn record_claude_count_tokens_outcome(outcome: &'static str) {
    metrics::counter!(
        "cc_switch_claude_count_tokens_total",
        "outcome" => outcome
    )
    .increment(1);
}

pub fn record_stream_transform_protocol_error(kind: &'static str) {
    metrics::counter!(
        "cc_switch_stream_transform_protocol_error_total",
        "kind" => kind
    )
    .increment(1);
}

pub fn record_stream_client_cancelled(app: &str) {
    metrics::counter!(
        "cc_switch_stream_client_cancelled_total",
        "app" => app.to_string()
    )
    .increment(1);
}

pub fn record_router_upgrade_task_report(outcome: &'static str) {
    metrics::counter!(
        "cc_switch_router_upgrade_task_reports_total",
        "outcome" => outcome
    )
    .increment(1);
    if outcome == "success" {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        metrics::gauge!("cc_switch_router_upgrade_task_report_last_success_timestamp_seconds")
            .set(timestamp);
    }
}

pub fn record_reasoning_bridge(direction: &'static str, outcome: &'static str) {
    metrics::counter!(
        "cc_switch_reasoning_bridge_total",
        "direction" => direction,
        "outcome" => outcome
    )
    .increment(1);
}

pub fn record_kimi_thinking_replay(outcome: &'static str, count: u64) {
    metrics::counter!(
        "cc_switch_kimi_thinking_replay_total",
        "outcome" => outcome
    )
    .increment(count);
}

pub fn record_proxy_semantic_guard(surface: &'static str, observation: &'static str) {
    metrics::counter!(
        "cc_switch_proxy_semantic_guard_total",
        "surface" => surface,
        "observation" => observation
    )
    .increment(1);
}

fn describe() {
    metrics::describe_gauge!(
        "cc_switch_account_inflight",
        "Current managed-account in-flight requests aggregated by provider type"
    );
    metrics::describe_counter!(
        "cc_switch_claude_retry_total",
        "Claude OAuth transparent retries by body stage and response source"
    );
    metrics::describe_counter!(
        "cc_switch_forward_retry_total",
        "Protocol-safe transparent forwarding retries by application, stage, and source"
    );
    metrics::describe_counter!(
        "cc_switch_codex_websocket_cache_total",
        "Codex WebSocket cache hits, misses, and releases"
    );
    metrics::describe_counter!(
        "cc_switch_codex_websocket_fallback_total",
        "Codex WebSocket to HTTP fallback attempts and outcomes"
    );
    metrics::describe_counter!(
        "cc_switch_codex_images_requests_total",
        "Codex OAuth Images requests by operation, response mode, and terminal outcome"
    );
    metrics::describe_counter!(
        "cc_switch_codex_images_output_total",
        "Codex OAuth Images outputs by operation and format"
    );
    metrics::describe_counter!(
        "cc_switch_codex_images_output_bytes_total",
        "Codex OAuth Images decoded output bytes by operation and format"
    );
    metrics::describe_counter!(
        "cc_switch_image_capability_events_total",
        "Image capability store insert, lookup, expiry, eviction, and integrity events"
    );
    metrics::describe_gauge!(
        "cc_switch_image_capability_entries",
        "Current durable image capability entry count"
    );
    metrics::describe_gauge!(
        "cc_switch_image_capability_bytes",
        "Current durable image capability payload bytes"
    );
    metrics::describe_histogram!(
        "cc_switch_image_transport_first_byte_seconds",
        "Time until the first downstream byte for long-running image transports"
    );
    metrics::describe_counter!(
        "cc_switch_image_transport_heartbeats_total",
        "Heartbeat chunks emitted for long-running image transports"
    );
    metrics::describe_histogram!(
        "cc_switch_image_transport_max_silence_seconds",
        "Maximum downstream silence observed during a long-running image transport"
    );
    metrics::describe_counter!(
        "cc_switch_provider_outcome_total",
        "Observed upstream outcomes aggregated by application and provider type"
    );
    metrics::describe_counter!(
        "cc_switch_account_warm_refresh_total",
        "Background managed-account token refresh results"
    );
    metrics::describe_counter!(
        "cc_switch_oauth_refresh_attempt_total",
        "Managed OAuth refresh attempts by bounded provider and outcome classification"
    );
    metrics::describe_histogram!(
        "cc_switch_oauth_refresh_attempt_duration_seconds",
        "Managed OAuth refresh attempt duration by bounded provider and outcome classification"
    );
    metrics::describe_counter!(
        "cc_switch_oauth_refresh_unknown_outcome_total",
        "Refresh attempts whose token rotation outcome cannot be established safely"
    );
    metrics::describe_counter!(
        "cc_switch_account_lease_total",
        "Managed account inference lease acquisitions and rejections"
    );
    metrics::describe_gauge!(
        "cc_switch_credential_persistence_degraded",
        "Whether rotated OAuth credentials are live but not durably persisted"
    );
    metrics::describe_counter!(
        "cc_switch_claude_cli_version_gate_total",
        "Claude CLI version gate responses rewritten for administrators"
    );
    metrics::describe_counter!(
        "cc_switch_grok_cli_version_gate_total",
        "Grok CLI version gate responses rewritten for administrators"
    );
    metrics::describe_counter!(
        "cc_switch_grok_model_catalog_total",
        "Grok model catalog responses by bounded source classification"
    );
    metrics::describe_counter!(
        "cc_switch_qoder_client_ip_total",
        "Qoder CN client IP resolutions by bounded source classification"
    );
    metrics::describe_counter!(
        "cc_switch_qoder_compatibility_total",
        "Qoder request and response compatibility normalizations by bounded kind"
    );
    metrics::describe_counter!(
        "cc_switch_qoder_error_total",
        "Qoder upstream failures by bounded protocol classification"
    );
    metrics::describe_counter!(
        "cc_switch_claude_bootstrap_total",
        "Best-effort Claude CLI bootstrap enrichment results"
    );
    metrics::describe_counter!(
        "cc_switch_claude_roles_total",
        "Best-effort bounded Claude CLI roles enrichment results"
    );
    metrics::describe_counter!(
        "cc_switch_claude_beta_decision_total",
        "Bounded Claude beta policy decisions"
    );
    metrics::describe_counter!(
        "cc_switch_claude_client_class_total",
        "Claude OAuth requests by fail-closed client class and operation"
    );
    metrics::describe_counter!(
        "cc_switch_claude_rate_limit_scope_total",
        "Claude OAuth 429 responses by bounded local cooldown scope"
    );
    metrics::describe_counter!(
        "cc_switch_claude_quota_header_observation_total",
        "Claude OAuth response-header quota observations by bounded commit outcome"
    );
    metrics::describe_counter!(
        "cc_switch_claude_optional_rewrite_total",
        "Bounded optional Claude wire rewrites"
    );
    metrics::describe_histogram!(
        "cc_switch_claude_ttfb_seconds",
        "Claude OAuth time to first semantic business output"
    );
    metrics::describe_histogram!(
        "cc_switch_claude_stream_duration_seconds",
        "Claude OAuth stream duration by bounded terminal classification"
    );
    metrics::describe_counter!(
        "cc_switch_claude_semantic_failure_total",
        "Claude OAuth semantic stream failures by bounded stage and classification"
    );
    metrics::describe_gauge!(
        "cc_switch_claude_wire_profile_info",
        "Active captured Claude Code wire profile"
    );
    metrics::describe_counter!(
        "cc_switch_claude_count_tokens_total",
        "Claude count-tokens request outcomes"
    );
    metrics::describe_counter!(
        "cc_switch_stream_transform_protocol_error_total",
        "Bounded cross-protocol stream transform errors"
    );
    metrics::describe_counter!(
        "cc_switch_stream_client_cancelled_total",
        "Downstream cancellations that stopped an upstream stream"
    );
    metrics::describe_counter!(
        "cc_switch_router_upgrade_task_reports_total",
        "Router upgrade task report attempts grouped by outcome"
    );
    metrics::describe_gauge!(
        "cc_switch_router_upgrade_task_report_last_success_timestamp_seconds",
        "Unix timestamp of the last successful Router upgrade task report"
    );
    metrics::describe_counter!(
        "cc_switch_reasoning_bridge_total",
        "Authenticated cross-protocol reasoning envelope operations"
    );
    metrics::describe_counter!(
        "cc_switch_kimi_thinking_replay_total",
        "Kimi signed-thinking replay cache outcomes without tenant identifiers"
    );
    metrics::describe_counter!(
        "cc_switch_proxy_semantic_guard_total",
        "Bounded Responses semantic classifications at downstream commit boundaries"
    );
    metrics::describe_counter!(
        "cc_switch_responses_sse_transport_total",
        "Bounded OpenAI Responses SSE normalization and liveness classifications"
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn prometheus_recorder_renders_registered_metrics() {
        super::init().unwrap();
        super::record_warm_refresh("claude_oauth", "success");
        super::record_account_inflight("claude_oauth", 2);
        super::record_provider_outcome(
            "claude",
            "claude_oauth",
            crate::domain::health::ProviderRequestOutcome::Success { status_code: 200 },
        );

        let output = super::render();
        assert!(output.contains("cc_switch_account_warm_refresh_total"));
        assert!(output.contains("provider_type=\"claude_oauth\""));
        assert!(output.contains("cc_switch_account_inflight{provider_type=\"claude_oauth\"} 2"));
        assert!(output.contains("cc_switch_provider_outcome_total"));
        for forbidden_label in ["account_id=", "provider_id=", "request_id="] {
            assert!(
                !output.contains(forbidden_label),
                "{forbidden_label}: {output}"
            );
        }
        assert!(!output.contains("cc_switch_account_max_concurrent"));
    }
}
