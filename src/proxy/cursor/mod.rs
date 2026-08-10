//! Cursor AgentService driver boundary.
//!
//! Cursor uses a provider-specific h2/protobuf driver rather than a normal HTTP
//! POST adapter. This module keeps the protocol pieces isolated from the Server
//! forwarder.

pub mod agent_driver;
pub mod agent_proto;
pub mod credential_cache;
pub mod event_emitter;
pub mod h2_client;
pub mod identity;
pub mod image;
pub mod model;
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

use super::adapters::AdapterRequest;
use super::router::ProxyRoute;
use super::{setting, ProxyError};

use protocol::CursorResponseFormat;
use request_builder::{build_plan, validate_tool_result_context, AgentRunPlan, InboundProtocol};

pub use agent_driver::{forward_agentservice, AgentServiceForwardOptions};

pub(super) fn apply_agentservice_model_selection(
    request: &mut AdapterRequest,
) -> Result<model::CursorModelResolution, ProxyError> {
    let explicit_selector = request
        .requested_model
        .as_deref()
        .filter(|model| model::is_explicit_cursor_selector(model))
        .map(str::to_string);
    let body_model = request_body_model(&request.body);
    let selected = explicit_selector
        .as_deref()
        .or(request.actual_model.as_deref())
        .or(request.model.as_deref())
        .or(body_model.as_deref())
        .unwrap_or("default")
        .to_string();
    let resolved = model::resolve_cursor_model(&selected).map_err(|message| {
        ProxyError::bad_request(format!(
            "invalid Cursor model selector `{selected}`: {message}"
        ))
    })?;
    request.body = replace_request_body_model(&request.body, &selected)?;
    request.model = Some(resolved.model_id.clone());
    request.actual_model = Some(resolved.model_id.clone());
    if explicit_selector.is_some() {
        request.actual_model_source = Some("cursor_explicit_selector".to_string());
    } else if selected != resolved.model_id {
        request.actual_model_source = Some("cursor_model_resolution".to_string());
    }
    Ok(resolved)
}

fn request_body_model(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()?
        .get("model")?
        .as_str()
        .map(str::to_string)
}

fn replace_request_body_model(body: &[u8], model: &str) -> Result<bytes::Bytes, ProxyError> {
    let mut value = serde_json::from_slice::<Value>(body).map_err(|error| {
        ProxyError::bad_request(format!("Cursor request body must be valid JSON: {error}"))
    })?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| ProxyError::bad_request("Cursor request body must be a JSON object"))?;
    object.insert("model".to_string(), Value::String(model.to_string()));
    serde_json::to_vec(&value)
        .map(bytes::Bytes::from)
        .map_err(|error| ProxyError::bad_request(format!("Cursor request encode failed: {error}")))
}

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
    use bytes::Bytes;
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
            resource: Default::default(),
        }
    }

    fn cursor_request(model: &str) -> AdapterRequest {
        super::super::adapters::cursor_agentservice_request(
            Bytes::from(
                serde_json::to_vec(&json!({
                    "model": model,
                    "messages": [{"role": "user", "content": "hi"}]
                }))
                .unwrap(),
            ),
            &stored(json!({})),
            ProxyRoute::ClaudeMessages,
            None,
        )
        .unwrap()
    }

    fn apply_single_model_result(request: &mut AdapterRequest, model: &str) {
        request.body = Bytes::from(
            serde_json::to_vec(&json!({
                "model": model,
                "messages": [{"role": "user", "content": "hi"}]
            }))
            .unwrap(),
        );
        request.model = Some(model.to_string());
        request.actual_model = Some(model.to_string());
        request.actual_model_source = Some("runtime_plan_single_model".to_string());
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

    #[test]
    fn explicit_cursor_selector_overrides_single_model_result() {
        let mut request = cursor_request("cursor-plan:gpt-5.5-fast");
        apply_single_model_result(&mut request, "composer-2.5");

        let resolved = apply_agentservice_model_selection(&mut request).unwrap();
        let body: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(body["model"], "cursor-plan:gpt-5.5-fast");
        assert_eq!(resolved.model_id, "gpt-5.5");
        assert_eq!(resolved.mode, model::CursorAgentMode::Plan);
        assert!(resolved.fast);
        assert_eq!(request.actual_model.as_deref(), Some("gpt-5.5"));
        assert_eq!(
            request.actual_model_source.as_deref(),
            Some("cursor_explicit_selector")
        );
    }

    #[test]
    fn composer_fast_alias_overrides_single_model_result() {
        let mut request = cursor_request("composer-2.5-fast");
        apply_single_model_result(&mut request, "gpt-5.5");

        let resolved = apply_agentservice_model_selection(&mut request).unwrap();
        let body: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(body["model"], "composer-2.5-fast");
        assert_eq!(resolved.model_id, "composer-2.5");
        assert_eq!(resolved.mode, model::CursorAgentMode::Agent);
        assert!(resolved.fast);
        assert_eq!(request.actual_model.as_deref(), Some("composer-2.5"));
        assert_eq!(
            request.actual_model_source.as_deref(),
            Some("cursor_explicit_selector")
        );
    }

    #[test]
    fn ordinary_model_keeps_single_model_result() {
        let mut request = cursor_request("gpt-5.5");
        apply_single_model_result(&mut request, "composer-2.5");

        let resolved = apply_agentservice_model_selection(&mut request).unwrap();
        let body: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(body["model"], "composer-2.5");
        assert_eq!(resolved.model_id, "composer-2.5");
        assert_eq!(request.requested_model.as_deref(), Some("gpt-5.5"));
        assert_eq!(
            request.actual_model_source.as_deref(),
            Some("runtime_plan_single_model")
        );
    }

    #[test]
    fn typed_cursor_oauth_explicit_disable_returns_fail_closed_contract() {
        let mut stored = stored(json!({
            "env": {
                "CURSOR_AGENT_SERVICE": "0",
                "CURSOR_AGENT_SERVICE_BASE_URL": "http://127.0.0.1:9"
            }
        }));
        stored.resource = crate::domain::providers::store::ProviderResourceMetadata {
            profile_id: Some(
                crate::domain::providers::registry::ProfileId::parse("claude.cursor_oauth")
                    .unwrap(),
            ),
            profile_schema_revision: Some(1),
            revision: 1,
            ..Default::default()
        };
        stored.provider.meta = Some(ProviderMeta {
            provider_type: Some("cursor_oauth".to_string()),
            auth_binding: Some(crate::domain::providers::model::AuthBinding {
                source: Some("managed_account".to_string()),
                auth_provider: Some("cursor_oauth".to_string()),
                account_id: Some("cursor-account".to_string()),
                auth_identity_generation: Some(1),
            }),
            ..Default::default()
        });

        assert_eq!(
            stored
                .resource
                .profile_id
                .as_ref()
                .map(|profile_id| profile_id.as_str()),
            Some("claude.cursor_oauth")
        );
        assert!(!agentservice_driver_requested(&stored));

        let body = serde_json::to_vec(&json!({
            "model": "composer-2.5",
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .unwrap();
        let error = agentservice_not_ready_error(ProxyRoute::ClaudeMessages, &stored, &body);

        assert_eq!(error.status, StatusCode::NOT_IMPLEMENTED);
        assert!(error.message.contains("native driver is disabled"));
        assert!(error.message.contains("provider=cursor-p"));
        assert!(error.message.contains("model=composer-2.5"));
        assert!(error.message.contains("protocol=anthropic_messages"));
    }
}
