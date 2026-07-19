//! Cursor AgentService migration boundary.
//!
//! The desktop implementation is a provider-specific h2/protobuf driver, not a
//! normal HTTP POST adapter. This module keeps the ported Cursor protocol
//! pieces isolated while the server forwarder grows a native driver path.

pub mod agent_driver;
pub mod agent_proto;
pub mod event_emitter;
pub mod h2_client;
pub mod identity;
pub mod image;
pub mod protocol;
pub mod request_builder;
pub mod session;
pub mod tool_bridge;
pub mod tool_resolver;

use axum::http::StatusCode;
use serde::Serialize;
use serde_json::Value;

use crate::domain::providers::model::ProviderType;
use crate::domain::providers::store::StoredProvider;

use super::router::ProxyRoute;
use super::{setting, ProxyError};

use protocol::CursorResponseFormat;
use request_builder::{build_plan, validate_tool_result_context, AgentRunPlan, InboundProtocol};

pub use agent_driver::{forward_agentservice, AgentServiceForwardOptions};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorAgentPlanPreview {
    pub provider_id: String,
    pub provider_type: ProviderType,
    pub inbound_protocol: &'static str,
    pub response_format: CursorResponseFormat,
    pub model_id: String,
    pub has_system_prompt: bool,
    pub tool_count: usize,
    pub image_count: usize,
    pub tool_result_count: usize,
    pub previous_response_id: Option<String>,
    pub working_directory: String,
}

pub fn is_cursor_provider(provider_type: ProviderType) -> bool {
    matches!(
        provider_type,
        ProviderType::CursorOAuth | ProviderType::CursorApiKey
    )
}

/// Cursor providers use the native h2 AgentService transport by default.
/// Operators can still disable it with provider env/settings while doing
/// upstream incident triage.
pub fn agentservice_driver_requested(stored: &StoredProvider) -> bool {
    if !is_cursor_provider(stored.provider_type) {
        return false;
    }
    if let Some(value) = setting(
        &stored.provider,
        &[
            "CURSOR_AGENT_SERVICE",
            "CURSOR_AGENTSERVICE",
            "CC_SWITCH_CURSOR_AGENT_SERVICE",
        ],
    ) {
        return truthy(&value);
    }
    if let Some(enabled) = stored
        .provider
        .settings_config
        .pointer("/cursorAgentService/enabled")
        .and_then(Value::as_bool)
        .or_else(|| {
            stored
                .provider
                .settings_config
                .pointer("/cursor_agent_service/enabled")
                .and_then(Value::as_bool)
        })
    {
        return enabled;
    }
    true
}

pub fn build_agent_plan_preview(
    route: ProxyRoute,
    stored: &StoredProvider,
    body: &[u8],
) -> Result<Option<CursorAgentPlanPreview>, ProxyError> {
    if !is_cursor_provider(stored.provider_type) {
        return Ok(None);
    }
    let Some((protocol, response_format, protocol_label)) = protocol_for_route(route) else {
        return Ok(None);
    };
    let value = serde_json::from_slice::<Value>(body).map_err(|error| {
        ProxyError::bad_request(format!("invalid cursor request JSON: {error}"))
    })?;
    let plan = build_plan(protocol, &value);
    validate_tool_result_context(&plan).map_err(|message| {
        ProxyError::bad_request(format!("invalid cursor tool result context: {message}"))
    })?;
    Ok(Some(plan_preview(
        stored,
        protocol_label,
        response_format,
        plan,
    )))
}

pub fn agentservice_not_ready_error(
    route: ProxyRoute,
    stored: &StoredProvider,
    body: &[u8],
) -> ProxyError {
    match build_agent_plan_preview(route, stored, body) {
        Ok(Some(preview)) => ProxyError {
            status: StatusCode::NOT_IMPLEMENTED,
            message: format!(
                "Cursor AgentService native driver is disabled for this provider; provider={}; model={}; protocol={}",
                preview.provider_id, preview.model_id, preview.inbound_protocol
            ),
        },
        Ok(None) => ProxyError {
            status: StatusCode::NOT_IMPLEMENTED,
            message: "Cursor AgentService native driver is staged but cannot handle this route yet".to_string(),
        },
        Err(error) => error,
    }
}

pub(super) fn protocol_for_route(
    route: ProxyRoute,
) -> Option<(InboundProtocol, CursorResponseFormat, &'static str)> {
    match route {
        ProxyRoute::ClaudeMessages => Some((
            InboundProtocol::AnthropicMessages,
            CursorResponseFormat::AnthropicMessages,
            "anthropic_messages",
        )),
        ProxyRoute::ClaudeCountTokens => None,
        ProxyRoute::CodexChatCompletions => Some((
            InboundProtocol::OpenAiChat,
            CursorResponseFormat::OpenAiChatCompletions,
            "openai_chat",
        )),
        ProxyRoute::CodexResponses | ProxyRoute::CodexResponsesCompact => Some((
            InboundProtocol::OpenAiResponses,
            CursorResponseFormat::OpenAiResponses,
            "openai_responses",
        )),
        ProxyRoute::Gemini => Some((
            InboundProtocol::GeminiNative,
            CursorResponseFormat::GeminiGenerateContent,
            "gemini_native",
        )),
    }
}

fn plan_preview(
    stored: &StoredProvider,
    inbound_protocol: &'static str,
    response_format: CursorResponseFormat,
    plan: AgentRunPlan,
) -> CursorAgentPlanPreview {
    CursorAgentPlanPreview {
        provider_id: stored.provider.id.clone(),
        provider_type: stored.provider_type,
        inbound_protocol,
        response_format,
        model_id: plan.model_id,
        has_system_prompt: plan
            .system_prompt
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty()),
        tool_count: plan.tools.len(),
        image_count: plan.images.len(),
        tool_result_count: plan.tool_results.len(),
        previous_response_id: plan.previous_response_id,
        working_directory: plan.working_directory,
    }
}

fn truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on" | "agentservice" | "agent_service"
    )
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use crate::domain::providers::model::{AppKind, Provider, ProviderMeta};

    use super::*;

    fn stored(settings_config: Value) -> StoredProvider {
        StoredProvider {
            app: AppKind::Claude,
            provider: Provider {
                id: "cursor-p".to_string(),
                name: "cursor".to_string(),
                settings_config,
                category: None,
                meta: Some(ProviderMeta {
                    provider_type: Some("cursor_oauth".to_string()),
                    ..Default::default()
                }),
                extra: Default::default(),
            },
            provider_type: ProviderType::CursorOAuth,
            provider_type_id: "cursor_oauth".to_string(),
        }
    }

    #[test]
    fn preview_builds_anthropic_plan_without_enabling_driver() {
        let body = serde_json::to_vec(&json!({
            "model": "composer-2.5",
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .unwrap();
        let preview =
            build_agent_plan_preview(ProxyRoute::ClaudeMessages, &stored(json!({})), &body)
                .unwrap()
                .unwrap();
        assert_eq!(preview.model_id, "composer-2.5");
        assert_eq!(preview.inbound_protocol, "anthropic_messages");
        assert_eq!(preview.tool_count, 0);
    }

    #[test]
    fn agentservice_driver_defaults_on_with_explicit_disable() {
        assert!(agentservice_driver_requested(&stored(json!({}))));
        assert!(agentservice_driver_requested(&stored(json!({
            "cursorAgentService": {"enabled": true}
        }))));
        assert!(!agentservice_driver_requested(&stored(json!({
            "cursorAgentService": {"enabled": false}
        }))));
    }
}
