//! Cursor Provider lifecycle facade.
//!
//! The protobuf/session implementation remains in `proxy::cursor`; this module
//! owns the Provider-specific preparation and dispatch boundary used by the
//! shared forwarder. Keeping that boundary here prevents the shared hot path
//! from learning Cursor adapter, model-selection, or h2 timeout details.

use std::time::Duration;

use axum::response::Response;
use bytes::Bytes;

use crate::domain::providers::store::StoredProvider;
use crate::domain::usage::store::UsageLogContext;
use crate::state::{AccountInFlightGuard, ServerState, ShareInFlightGuard};

use super::super::adapters::{self, AdapterRequest};
use super::super::cursor as driver;
use super::super::provider_ops::ProviderExecution;
use super::super::request_memory::RequestMemoryBudget;
use super::super::router::ProxyRoute;
use super::super::ProxyError;

pub(crate) struct PreparedAgentServiceRequest {
    pub(crate) adapter_request: AdapterRequest,
    pub(crate) model_id: String,
}

pub(crate) fn agentservice_driver_requested(stored: &StoredProvider) -> bool {
    driver::agentservice_driver_requested(stored)
}

pub(crate) fn prepare_agentservice_request(
    execution: &ProviderExecution,
    stored: &StoredProvider,
    route: ProxyRoute,
    gemini_path: Option<&str>,
    body: Bytes,
) -> Result<PreparedAgentServiceRequest, ProxyError> {
    let mut adapter_request =
        adapters::cursor_agentservice_request(body, stored, route, gemini_path)?;
    execution.enforce_model_policy(&mut adapter_request)?;
    let resolution = driver::apply_agentservice_model_selection(&mut adapter_request)?;
    Ok(PreparedAgentServiceRequest {
        adapter_request,
        model_id: resolution.model_id,
    })
}

pub(crate) fn agentservice_not_ready_error(
    route: ProxyRoute,
    stored: &StoredProvider,
    body: &[u8],
) -> ProxyError {
    driver::agentservice_not_ready_error(route, stored, body)
}

pub(crate) struct AgentServiceForwardOptions {
    pub(crate) state: ServerState,
    pub(crate) route: ProxyRoute,
    pub(crate) stored: StoredProvider,
    pub(crate) adapter_request: AdapterRequest,
    pub(crate) request_context: UsageLogContext,
    pub(crate) account_in_flight_guard: Option<AccountInFlightGuard>,
    pub(crate) share_invocation_guard: Option<ShareInFlightGuard>,
    pub(crate) runtime_fingerprint: String,
    pub(crate) request_timeout: Duration,
    pub(crate) first_frame_timeout: Option<Duration>,
    pub(crate) inter_frame_timeout: Option<Duration>,
    pub(crate) request_memory: Option<RequestMemoryBudget>,
}

pub(crate) async fn forward_agentservice(
    options: AgentServiceForwardOptions,
) -> Result<Response, ProxyError> {
    driver::forward_agentservice(driver::AgentServiceForwardOptions {
        state: options.state,
        route: options.route,
        stored: options.stored,
        adapter_request: options.adapter_request,
        request_context: options.request_context,
        account_in_flight_guard: options.account_in_flight_guard,
        share_invocation_guard: options.share_invocation_guard,
        runtime_fingerprint: options.runtime_fingerprint,
        timeouts: driver::h2_client::CursorH2Timeouts {
            request: options.request_timeout,
            first_frame: options.first_frame_timeout,
            inter_frame: options.inter_frame_timeout,
        },
        request_memory: options.request_memory,
    })
    .await
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use crate::domain::providers::model::{AppKind, Provider, ProviderMeta, ProviderType};

    use super::*;

    fn stored(provider_type: ProviderType, settings_config: Value) -> StoredProvider {
        StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: format!("{}-provider", provider_type.as_str()),
                name: "Cursor fixture".to_string(),
                settings_config,
                category: None,
                meta: Some(ProviderMeta {
                    provider_type: Some(provider_type.as_str().to_string()),
                    ..Default::default()
                }),
                extra: Default::default(),
            },
            provider_type,
            provider_type_id: provider_type.as_str().to_string(),
            resource: Default::default(),
        }
    }

    #[test]
    fn cursor_provider_facade_keeps_rails_independent_and_fail_closed() {
        let oauth = stored(
            ProviderType::CursorOAuth,
            json!({"env": {"CURSOR_OAUTH_AGENT_SERVICE": "0"}}),
        );
        let api_key = stored(
            ProviderType::CursorApiKey,
            json!({"env": {"CURSOR_APIKEY_AGENT_SERVICE": "1"}}),
        );

        assert!(!agentservice_driver_requested(&oauth));
        assert!(agentservice_driver_requested(&api_key));

        let body = serde_json::to_vec(&json!({
            "model": "composer-2.5",
            "messages": [{"role": "user", "content": "fixture"}]
        }))
        .unwrap();
        let error = agentservice_not_ready_error(ProxyRoute::CodexChatCompletions, &oauth, &body);
        assert_eq!(error.status, axum::http::StatusCode::NOT_IMPLEMENTED);
        assert!(error.message.contains("native driver is disabled"));
        assert!(!error.message.contains(api_key.provider.id.as_str()));
    }
}
