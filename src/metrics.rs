use std::sync::OnceLock;

use anyhow::Context;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

use crate::domain::health::ProviderRequestOutcome;

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

fn set_claude_wire_profile_info() {
    let profile = crate::domain::claude_cli::CLAUDE_WIRE_PROFILE;
    metrics::gauge!(
        "cc_switch_claude_wire_profile_info",
        "profile_id" => profile.id,
        "claude_code_version" => profile.claude_code_version,
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

pub fn record_reasoning_bridge(direction: &'static str, outcome: &'static str) {
    metrics::counter!(
        "cc_switch_reasoning_bridge_total",
        "direction" => direction,
        "outcome" => outcome
    )
    .increment(1);
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
        "cc_switch_reasoning_bridge_total",
        "Authenticated cross-protocol reasoning envelope operations"
    );
    metrics::describe_counter!(
        "cc_switch_proxy_semantic_guard_total",
        "Bounded Responses semantic classifications at downstream commit boundaries"
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
