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
    Ok(())
}

pub fn render() -> String {
    PROMETHEUS_HANDLE
        .get()
        .map(PrometheusHandle::render)
        .unwrap_or_default()
}

pub fn record_account_inflight(
    provider_type: &str,
    account_id: &str,
    current: u32,
    max_concurrent: u32,
) {
    metrics::gauge!(
        "cc_switch_account_inflight",
        "provider_type" => provider_type.to_string(),
        "account_id" => account_id.to_string()
    )
    .set(f64::from(current));
    metrics::gauge!(
        "cc_switch_account_max_concurrent",
        "provider_type" => provider_type.to_string(),
        "account_id" => account_id.to_string()
    )
    .set(f64::from(max_concurrent));
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

pub fn record_provider_outcome(app: &str, provider_id: &str, outcome: ProviderRequestOutcome) {
    let outcome = match outcome {
        ProviderRequestOutcome::Success { .. } => "success",
        ProviderRequestOutcome::Failure { .. } => "failure",
        ProviderRequestOutcome::RateLimited { .. } => "rate_limited",
        ProviderRequestOutcome::NetworkFailure => "network_failure",
    };
    metrics::counter!(
        "cc_switch_provider_outcome_total",
        "app" => app.to_string(),
        "provider_id" => provider_id.to_string(),
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

pub fn record_claude_cli_version_gate() {
    metrics::counter!("cc_switch_claude_cli_version_gate_total").increment(1);
}

pub fn record_claude_bootstrap(result: &str) {
    metrics::counter!(
        "cc_switch_claude_bootstrap_total",
        "result" => result.to_string()
    )
    .increment(1);
}

pub fn record_claude_beta_decision(decision: &'static str) {
    metrics::counter!(
        "cc_switch_claude_beta_decision_total",
        "decision" => decision
    )
    .increment(1);
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

fn describe() {
    metrics::describe_gauge!(
        "cc_switch_account_inflight",
        "Current in-flight requests for a managed account"
    );
    metrics::describe_gauge!(
        "cc_switch_account_max_concurrent",
        "Configured maximum concurrent requests for a managed account"
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
        "cc_switch_provider_outcome_total",
        "Observed upstream outcomes for each provider"
    );
    metrics::describe_counter!(
        "cc_switch_account_warm_refresh_total",
        "Background managed-account token refresh results"
    );
    metrics::describe_counter!(
        "cc_switch_claude_cli_version_gate_total",
        "Claude CLI version gate responses rewritten for administrators"
    );
    metrics::describe_counter!(
        "cc_switch_claude_bootstrap_total",
        "Best-effort Claude CLI bootstrap enrichment results"
    );
    metrics::describe_counter!(
        "cc_switch_claude_beta_decision_total",
        "Bounded Claude beta policy decisions"
    );
    metrics::describe_counter!(
        "cc_switch_claude_count_tokens_total",
        "Claude count-tokens request outcomes"
    );
    metrics::describe_counter!(
        "cc_switch_stream_transform_protocol_error_total",
        "Bounded cross-protocol stream transform errors"
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn prometheus_recorder_renders_registered_metrics() {
        super::init().unwrap();
        super::record_warm_refresh("claude_oauth", "success");

        let output = super::render();
        assert!(output.contains("cc_switch_account_warm_refresh_total"));
        assert!(output.contains("provider_type=\"claude_oauth\""));
    }
}
