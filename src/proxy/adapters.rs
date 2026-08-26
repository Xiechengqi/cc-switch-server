use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use super::cache_injector::{inject_prompt_cache, CacheInjectionConfig};
use super::copilot_model_map::normalize_or_resolve_model;
pub(super) use super::copilot_optimizer::CopilotRequestMetadata;
use super::copilot_optimizer::{
    optimize_request as optimize_copilot_request, CopilotOptimizerConfig,
};
use super::request_governance::{govern_request_body, RequestGovernanceConfig};
use super::thinking::{apply_thinking_pipeline, ThinkingPipelineConfig};
use super::tool_schema::normalize_gemini_tool_schemas;
use super::{join_url, setting, transforms, ProxyError, ProxyRoute};
use crate::domain::accounts::managers::{manager_for, AccountManager, CredentialKind};
use crate::domain::accounts::store::{Account, AccountStore};
use crate::domain::providers::model::{AppKind, ProviderType};
use crate::domain::providers::model_routing::{policy_from_settings, ModelRoutingMode};
use crate::domain::providers::registry::UpstreamProtocol;
use crate::domain::providers::runtime::{
    managed_account_binding, managed_account_provider_type, ProviderRuntimePlan,
};
use crate::domain::providers::store::StoredProvider;
use crate::domain::usage::store::{
    usage_from_json_with_semantics, InputTokenSemantics, TokenUsage,
};
use bytes::Bytes;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum AdapterSupport {
    Native,
    GenericFallback,
    Planned,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterCapability {
    pub app: AppKind,
    pub provider_type: ProviderType,
    pub adapter: &'static str,
    pub support: AdapterSupport,
    pub native_format: &'static str,
    pub requires_transform: bool,
    pub supports_stream_usage: bool,
    pub supports_oauth_refresh: bool,
    pub supports_model_list: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_responses_protocol: Option<super::protocol_compat::ResponsesProtocolCapability>,
}

pub trait ProviderAdapter {
    fn capability(&self, app: AppKind, provider_type: ProviderType) -> AdapterCapability;
    fn resolve_endpoint(
        &self,
        route: ProxyRoute,
        gemini_path: Option<String>,
        stored: &StoredProvider,
    ) -> Result<String, ProxyError>;
    fn build_headers(
        &self,
        app: AppKind,
        stored: &StoredProvider,
        accounts: &AccountStore,
    ) -> Result<Vec<(&'static str, String)>, ProxyError>;
    fn transform_request(
        &self,
        body: Bytes,
        stored: &StoredProvider,
    ) -> Result<AdapterRequest, ProxyError>;
    fn transform_response(
        &self,
        body: Bytes,
        stored: &StoredProvider,
        route: ProxyRoute,
    ) -> Result<Bytes, ProxyError>;
    fn transform_stream_event(
        &self,
        chunk: Bytes,
        stored: &StoredProvider,
        route: ProxyRoute,
    ) -> Result<Bytes, ProxyError>;
    fn parse_usage(&self, body: &[u8], stored: &StoredProvider, route: ProxyRoute) -> TokenUsage;
}

#[derive(Debug, Clone, Copy)]
pub struct GenericForwardingAdapter {
    profile: AdapterProfile,
}

#[derive(Debug, Clone, Copy)]
struct AdapterProfile {
    adapter: &'static str,
    support: AdapterSupport,
}

pub struct AdapterRequest {
    pub body: Bytes,
    pub upstream_endpoint: Option<String>,
    pub upstream_headers: Vec<(&'static str, String)>,
    pub model: Option<String>,
    pub requested_model: Option<String>,
    pub actual_model: Option<String>,
    pub actual_model_source: Option<String>,
    pub(super) gemini_action: Option<GeminiRouteAction>,
    /// Whether the downstream client expects a streaming response.
    pub stream_requested: bool,
    /// Whether the selected upstream transport must be streaming.
    pub upstream_stream_requested: bool,
    pub custom_tool_names: BTreeSet<String>,
    pub(crate) responses_tool_context: transforms::ResponsesToolContext,
    pub claude_tool_name_map: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GeminiRouteAction {
    GenerateContent,
    StreamGenerateContent,
    CountTokens,
}

impl AdapterRequest {
    pub(super) fn is_gemini_count_tokens(&self) -> bool {
        self.gemini_action == Some(GeminiRouteAction::CountTokens)
    }
}

type CopilotPreflightResult = (Bytes, Vec<(&'static str, String)>, Option<&'static str>);

impl ProviderAdapter for GenericForwardingAdapter {
    fn capability(&self, app: AppKind, provider_type: ProviderType) -> AdapterCapability {
        adapter_capability(app, provider_type, self.profile)
    }

    fn resolve_endpoint(
        &self,
        route: ProxyRoute,
        gemini_path: Option<String>,
        stored: &StoredProvider,
    ) -> Result<String, ProxyError> {
        let base_url = base_url(route.app(), stored)?;
        Ok(join_upstream_url(&base_url, &route.path(gemini_path)))
    }

    fn build_headers(
        &self,
        app: AppKind,
        stored: &StoredProvider,
        accounts: &AccountStore,
    ) -> Result<Vec<(&'static str, String)>, ProxyError> {
        let mut headers = Vec::new();
        let header_app = header_app_for(app, stored.provider_type);
        if stored.resource.profile_id.is_none() && stored.provider_type != ProviderType::AwsBedrock
        {
            apply_auth_headers(&mut headers, header_app, stored, accounts)?;
        }

        if header_app == AppKind::Claude
            && !matches!(
                stored.provider_type,
                ProviderType::AwsBedrock | ProviderType::GitHubCopilot
            )
        {
            headers.push(("anthropic-version", "2023-06-01".to_string()));
        }

        Ok(headers)
    }

    fn transform_request(
        &self,
        body: Bytes,
        stored: &StoredProvider,
    ) -> Result<AdapterRequest, ProxyError> {
        self.transform_request_inner(body, stored, None, None)
    }

    fn transform_response(
        &self,
        body: Bytes,
        stored: &StoredProvider,
        route: ProxyRoute,
    ) -> Result<Bytes, ProxyError> {
        transform_response_for_downstream(
            body,
            stored,
            route,
            &transforms::ResponsesToolContext::default(),
        )
    }

    fn transform_stream_event(
        &self,
        chunk: Bytes,
        stored: &StoredProvider,
        route: ProxyRoute,
    ) -> Result<Bytes, ProxyError> {
        Ok(transform_stream_event_for_downstream(
            chunk,
            stored,
            route,
            &transforms::ResponsesToolContext::default(),
        ))
    }

    fn parse_usage(&self, body: &[u8], stored: &StoredProvider, route: ProxyRoute) -> TokenUsage {
        serde_json::from_slice::<Value>(body)
            .map(|value| {
                usage_from_json_with_semantics(&value, usage_input_semantics_for(stored, route))
            })
            .unwrap_or_default()
    }
}

pub(super) fn usage_input_semantics_for(
    stored: &StoredProvider,
    route: ProxyRoute,
) -> InputTokenSemantics {
    let format = upstream_format_for_route(stored, Some(route), &[])
        .unwrap_or_else(|| downstream_format_for_route(route));
    match format {
        UpstreamFormat::AnthropicMessages => InputTokenSemantics::Exclusive,
        UpstreamFormat::OpenAiChat
        | UpstreamFormat::OpenAiResponses
        | UpstreamFormat::GeminiNative => InputTokenSemantics::Inclusive,
    }
}

impl GenericForwardingAdapter {
    fn transform_request_inner(
        &self,
        body: Bytes,
        stored: &StoredProvider,
        route: Option<ProxyRoute>,
        copilot_metadata: Option<&CopilotRequestMetadata>,
    ) -> Result<AdapterRequest, ProxyError> {
        let responses_tool_context = transforms::responses_tool_context_from_bytes(&body);
        let custom_tool_names = responses_tool_context.custom_tool_names();
        let downstream_stream_requested = is_stream_requested(&body);
        let cache_config = cache_injection_config(stored);
        let thinking_config = thinking_pipeline_config(stored);
        let governance_config = request_governance_config(stored);
        let body = maybe_inject_downstream_prompt_cache(body, stored, route, &cache_config)?;
        let (body, model, upstream_headers) = if stored.provider_type == ProviderType::GitHubCopilot
        {
            let (body, mut model) = apply_request_preprocessors(body, stored, route);
            let (mut body, upstream_headers, model_source) =
                maybe_apply_copilot_preflight(body, stored, copilot_metadata)?;
            if policy_from_settings(&stored.provider.settings_config).is_some() {
                let requested_model = model.requested_model.clone();
                let (routed_body, mut routed_model) =
                    apply_request_preprocessors(body, stored, route);
                routed_model.requested_model = requested_model;
                body = routed_body;
                model = routed_model;
            } else if let Some(source) = model_source {
                if let Some(actual_model) = model_from_body(&body) {
                    model.actual_model = Some(actual_model.clone());
                    model.actual_model_source = Some(source.to_string());
                }
            }
            (
                transform_body_for_upstream(body, stored, route)?,
                model,
                upstream_headers,
            )
        } else {
            let (body, model) = apply_request_preprocessors(body, stored, route);
            let body = transform_body_for_upstream(body, stored, route)?;
            (body, model, Vec::new())
        };
        let body = maybe_apply_request_governance(body, stored, &governance_config)?;
        let body = maybe_apply_thinking_pipeline(body, stored, route, &thinking_config)?;
        let body = maybe_inject_upstream_prompt_cache(body, stored, route, &cache_config)?;
        let stream_requested = downstream_stream_requested || is_stream_requested(&body);
        Ok(AdapterRequest {
            body,
            upstream_endpoint: None,
            model: model
                .actual_model
                .clone()
                .or_else(|| model.requested_model.clone()),
            requested_model: model.requested_model,
            actual_model: model.actual_model.clone(),
            actual_model_source: model.actual_model_source,
            gemini_action: None,
            stream_requested,
            upstream_stream_requested: stream_requested,
            upstream_headers,
            custom_tool_names,
            responses_tool_context,
            claude_tool_name_map: Default::default(),
        })
    }

    pub(crate) fn transform_request_for_route(
        &self,
        body: Bytes,
        stored: &StoredProvider,
        route: ProxyRoute,
        gemini_path: Option<&str>,
    ) -> Result<AdapterRequest, ProxyError> {
        let body =
            maybe_inject_gemini_cross_protocol_route_model(body, stored, route, gemini_path)?;
        let mut request = self.transform_request_inner(body, stored, Some(route), None)?;
        self.finish_route_request(&mut request, stored, route, gemini_path)?;
        Ok(request)
    }

    pub(super) fn transform_request_for_route_with_metadata(
        &self,
        body: Bytes,
        stored: &StoredProvider,
        route: ProxyRoute,
        gemini_path: Option<&str>,
        copilot_metadata: &CopilotRequestMetadata,
    ) -> Result<AdapterRequest, ProxyError> {
        let body =
            maybe_inject_gemini_cross_protocol_route_model(body, stored, route, gemini_path)?;
        let mut request =
            self.transform_request_inner(body, stored, Some(route), Some(copilot_metadata))?;
        self.finish_route_request(&mut request, stored, route, gemini_path)?;
        Ok(request)
    }

    fn finish_route_request(
        &self,
        request: &mut AdapterRequest,
        stored: &StoredProvider,
        route: ProxyRoute,
        gemini_path: Option<&str>,
    ) -> Result<(), ProxyError> {
        prepare_gemini_v1internal_route_request(request, stored, route, gemini_path)?;
        if route_implies_stream(route, gemini_path) {
            request.stream_requested = true;
            request.upstream_stream_requested = true;
            if let Some(upstream_format) =
                upstream_format_for_route(stored, Some(route), &request.body)
            {
                request.body = ensure_stream_enabled(request.body.clone(), upstream_format)?;
            }
        }
        if is_gemini_v1internal_provider_type(stored.provider_type) {
            let count_tokens = request.is_gemini_count_tokens();
            request.stream_requested &= !count_tokens;
            request.upstream_stream_requested = !count_tokens;
        }
        apply_bedrock_forward_contract(stored, request)
    }

    pub fn resolve_endpoint_for_request(
        &self,
        route: ProxyRoute,
        gemini_path: Option<String>,
        stored: &StoredProvider,
        request: &AdapterRequest,
    ) -> Result<String, ProxyError> {
        if let Some(endpoint) = request.upstream_endpoint.as_ref() {
            return Ok(endpoint.clone());
        }
        let Some(upstream_format) = upstream_format_for_route(stored, Some(route), &request.body)
        else {
            return self.resolve_endpoint(route, gemini_path, stored);
        };
        let base_url = base_url_for_upstream(upstream_format, stored)?;
        Ok(join_upstream_url(
            &base_url,
            &upstream_path_for_provider(stored, upstream_format, route, gemini_path, request),
        ))
    }

    pub(super) fn transform_response_for_request(
        &self,
        body: Bytes,
        stored: &StoredProvider,
        route: ProxyRoute,
        request: &AdapterRequest,
    ) -> Result<Bytes, ProxyError> {
        transform_response_for_downstream(body, stored, route, &request.responses_tool_context)
    }

    pub(super) fn transform_stream_event_for_request(
        &self,
        chunk: Bytes,
        stored: &StoredProvider,
        route: ProxyRoute,
        responses_tool_context: &transforms::ResponsesToolContext,
    ) -> Result<Bytes, ProxyError> {
        Ok(transform_stream_event_for_downstream(
            chunk,
            stored,
            route,
            responses_tool_context,
        ))
    }
}

pub(super) fn resolve_runtime_endpoint_for_request(
    plan: &ProviderRuntimePlan,
    route: ProxyRoute,
    gemini_path: Option<String>,
    stored: &StoredProvider,
    request: &AdapterRequest,
) -> Result<String, ProxyError> {
    if let Some(endpoint) = request.upstream_endpoint.as_ref() {
        return Ok(endpoint.clone());
    }
    if is_gemini_v1internal_provider_type(stored.provider_type) {
        if request.is_gemini_count_tokens() {
            let model = gemini_v1internal_model(request)?;
            return Ok(gemini_ai_studio_count_tokens_endpoint(
                GEMINI_AI_STUDIO_BASE_URL,
                model,
            ));
        }
        return Ok(gemini_v1internal_endpoint(&plan.endpoint));
    }
    if plan.upstream_protocol == UpstreamProtocol::Legacy {
        return adapter_for(stored.app, stored.provider_type).resolve_endpoint_for_request(
            route,
            gemini_path,
            stored,
            request,
        );
    }
    if plan.driver_id.as_str() == "aws.bedrock_sigv4" {
        return Ok(plan.endpoint.clone());
    }
    let upstream_format = runtime_upstream_format(plan).ok_or_else(|| {
        ProxyError::bad_request(format!(
            "driver {} must provide a specialized upstream endpoint",
            plan.driver_id
        ))
    })?;
    let path = if plan.driver_id.as_str() == "oauth.kimi_code" {
        kimi_upstream_path(upstream_format, route)
    } else if plan.driver_id.as_str() == "special.copilot"
        && upstream_format == UpstreamFormat::OpenAiChat
    {
        "/chat/completions".to_string()
    } else {
        upstream_path(upstream_format, route, gemini_path, request)
    };
    Ok(join_upstream_url(&plan.endpoint, &path))
}

pub(super) fn runtime_model_list_url(plan: &ProviderRuntimePlan) -> Option<String> {
    if plan.coding_plan.is_some() {
        return None;
    }
    if plan.driver_id.as_str() == "oauth.kimi_code" {
        return Some(join_upstream_url(&plan.endpoint, "/v1/models"));
    }
    let path = match plan.upstream_protocol {
        UpstreamProtocol::AnthropicMessages
        | UpstreamProtocol::OpenAiChat
        | UpstreamProtocol::OpenAiResponses => "/v1/models",
        UpstreamProtocol::GeminiNative => "/v1beta/models",
        _ => return None,
    };
    Some(join_upstream_url(&plan.endpoint, path))
}

fn runtime_upstream_format(plan: &ProviderRuntimePlan) -> Option<UpstreamFormat> {
    match plan.upstream_protocol {
        UpstreamProtocol::AnthropicMessages => Some(UpstreamFormat::AnthropicMessages),
        UpstreamProtocol::OpenAiChat => Some(UpstreamFormat::OpenAiChat),
        UpstreamProtocol::OpenAiResponses => Some(UpstreamFormat::OpenAiResponses),
        UpstreamProtocol::GeminiNative => Some(UpstreamFormat::GeminiNative),
        UpstreamProtocol::Special => match plan.driver_id.as_str() {
            "special.cursor" | "special.copilot" | "special.qoder_cosy" => {
                Some(UpstreamFormat::OpenAiChat)
            }
            "special.antigravity" | "special.agy" => Some(UpstreamFormat::GeminiNative),
            "oauth.kimi_code" => Some(match plan.provider_key.app {
                AppKind::Claude => UpstreamFormat::AnthropicMessages,
                AppKind::Codex | AppKind::Gemini => UpstreamFormat::OpenAiChat,
            }),
            _ => None,
        },
        UpstreamProtocol::Bedrock | UpstreamProtocol::Custom | UpstreamProtocol::Legacy => None,
    }
}

pub(crate) fn cursor_agentservice_request(
    body: Bytes,
    stored: &StoredProvider,
    route: ProxyRoute,
    gemini_path: Option<&str>,
) -> Result<AdapterRequest, ProxyError> {
    let body = maybe_inject_gemini_route_model(body, route, gemini_path)?;
    let downstream_stream_requested =
        is_stream_requested(&body) || route_implies_stream(route, gemini_path);
    let governance_config = request_governance_config(stored);
    let responses_tool_context = transforms::responses_tool_context_from_bytes(&body);
    let custom_tool_names = responses_tool_context.custom_tool_names();
    let (body, model) = apply_request_preprocessors(body, stored, Some(route));
    let body = maybe_apply_request_governance(body, stored, &governance_config)?;
    let stream_requested = downstream_stream_requested || is_stream_requested(&body);
    Ok(AdapterRequest {
        body,
        upstream_endpoint: None,
        upstream_headers: Vec::new(),
        model: model
            .actual_model
            .clone()
            .or_else(|| model.requested_model.clone()),
        requested_model: model.requested_model,
        actual_model: model.actual_model.clone(),
        actual_model_source: model.actual_model_source,
        gemini_action: None,
        stream_requested,
        upstream_stream_requested: stream_requested,
        custom_tool_names,
        responses_tool_context,
        claude_tool_name_map: Default::default(),
    })
}

fn maybe_inject_gemini_route_model(
    body: Bytes,
    route: ProxyRoute,
    gemini_path: Option<&str>,
) -> Result<Bytes, ProxyError> {
    if route != ProxyRoute::Gemini {
        return Ok(body);
    }
    let Some(model) = gemini_model_from_path(gemini_path) else {
        return Ok(body);
    };
    let mut value = serde_json::from_slice::<Value>(&body).map_err(|error| {
        ProxyError::bad_request(format!(
            "Gemini request body must be valid json for route model injection: {error}"
        ))
    })?;
    let Value::Object(map) = &mut value else {
        return Ok(body);
    };
    map.entry("model".to_string())
        .or_insert(Value::String(model));
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|error| {
            ProxyError::bad_request(format!("Gemini Cursor request encode failed: {error}"))
        })
}

fn maybe_inject_gemini_cross_protocol_route_model(
    body: Bytes,
    stored: &StoredProvider,
    route: ProxyRoute,
    gemini_path: Option<&str>,
) -> Result<Bytes, ProxyError> {
    if !matches!(
        upstream_format_for_route(stored, Some(route), &body),
        Some(
            UpstreamFormat::AnthropicMessages
                | UpstreamFormat::OpenAiResponses
                | UpstreamFormat::OpenAiChat
        )
    ) {
        return Ok(body);
    }
    maybe_inject_gemini_route_model(body, route, gemini_path)
}

fn gemini_model_from_path(gemini_path: Option<&str>) -> Option<String> {
    let path = gemini_path?.trim();
    let after_models = path.strip_prefix("models/").unwrap_or(path);
    let model = after_models
        .split(':')
        .next()
        .unwrap_or(after_models)
        .trim();
    (!model.is_empty()).then(|| model.to_string())
}

pub fn adapter_for(app: AppKind, provider_type: ProviderType) -> GenericForwardingAdapter {
    GenericForwardingAdapter {
        profile: adapter_profile(app, provider_type),
    }
}

pub fn capability_for(app: AppKind, provider_type: ProviderType) -> AdapterCapability {
    adapter_for(app, provider_type).capability(app, provider_type)
}

pub fn all_capabilities() -> Vec<AdapterCapability> {
    [AppKind::Claude, AppKind::Codex, AppKind::Gemini]
        .into_iter()
        .flat_map(|app| {
            all_provider_types().map(move |provider_type| capability_for(app, provider_type))
        })
        .collect()
}

fn all_provider_types() -> impl Iterator<Item = ProviderType> {
    [
        ProviderType::Claude,
        ProviderType::ClaudeAuth,
        ProviderType::ClaudeOAuth,
        ProviderType::Codex,
        ProviderType::CodexOAuth,
        ProviderType::Gemini,
        ProviderType::GeminiCli,
        ProviderType::OpenRouter,
        ProviderType::GitHubCopilot,
        ProviderType::DeepSeekAccount,
        ProviderType::KiroOAuth,
        ProviderType::KimiCode,
        ProviderType::QoderCosy,
        ProviderType::CursorOAuth,
        ProviderType::CursorApiKey,
        ProviderType::AntigravityOAuth,
        ProviderType::AgyOAuth,
        ProviderType::OllamaCloud,
        ProviderType::AwsBedrock,
        ProviderType::Nvidia,
        ProviderType::DeepSeekApi,
        ProviderType::GrokOAuth,
    ]
    .into_iter()
}

fn adapter_capability(
    app: AppKind,
    provider_type: ProviderType,
    profile: AdapterProfile,
) -> AdapterCapability {
    AdapterCapability {
        app,
        provider_type,
        adapter: profile.adapter,
        support: profile.support,
        native_format: native_format(app),
        requires_transform: requires_transform(app, provider_type),
        supports_stream_usage: supports_stream_usage(app, provider_type),
        supports_oauth_refresh: false,
        supports_model_list: supports_model_list(app, provider_type),
        upstream_responses_protocol: (provider_type == ProviderType::GrokOAuth)
            .then(super::protocol_compat::grok_responses_capability),
    }
}

fn adapter_profile(app: AppKind, provider_type: ProviderType) -> AdapterProfile {
    let (adapter, support) = match (app, provider_type) {
        (AppKind::Claude, ProviderType::Claude) => ("claude_anthropic_api", AdapterSupport::Native),
        (AppKind::Claude, ProviderType::ClaudeAuth) => {
            ("claude_bearer_compatible", AdapterSupport::Native)
        }
        (AppKind::Claude, ProviderType::ClaudeOAuth) => {
            ("claude_oauth_bearer_compatible", AdapterSupport::Native)
        }
        (AppKind::Claude, ProviderType::Codex) => {
            ("claude_to_codex_responses", AdapterSupport::Native)
        }
        (AppKind::Claude, ProviderType::CodexOAuth) => {
            ("claude_to_codex_oauth_responses", AdapterSupport::Native)
        }
        (AppKind::Claude, ProviderType::Gemini | ProviderType::GeminiCli) => {
            ("claude_to_gemini_native", AdapterSupport::Native)
        }
        (AppKind::Claude, ProviderType::OpenRouter) => {
            ("claude_openrouter_compatible", AdapterSupport::Native)
        }
        (AppKind::Claude, ProviderType::GitHubCopilot) => {
            ("claude_to_copilot_chat", AdapterSupport::Native)
        }
        (AppKind::Claude, ProviderType::DeepSeekAccount) => {
            planned("claude_deepseek_account_planned")
        }
        (AppKind::Claude, ProviderType::KiroOAuth) => {
            ("claude_kiro_codewhisperer", AdapterSupport::Native)
        }
        (AppKind::Claude, ProviderType::CursorOAuth) => {
            ("claude_cursor_agentservice", AdapterSupport::Native)
        }
        (AppKind::Claude, ProviderType::CursorApiKey) => {
            ("claude_cursor_apikey_agentservice", AdapterSupport::Native)
        }
        (AppKind::Claude, ProviderType::AntigravityOAuth | ProviderType::AgyOAuth) => {
            ("claude_antigravity_gemini_native", AdapterSupport::Native)
        }
        (AppKind::Claude, ProviderType::OllamaCloud) => {
            ("claude_ollama_openai_chat", AdapterSupport::Native)
        }
        (AppKind::Claude, ProviderType::AwsBedrock) => planned("claude_bedrock_signature_planned"),
        (AppKind::Claude, ProviderType::Nvidia) => {
            ("claude_nvidia_openai_chat", AdapterSupport::Native)
        }
        (AppKind::Claude, ProviderType::DeepSeekApi) => {
            ("claude_deepseek_anthropic_api", AdapterSupport::Native)
        }
        (AppKind::Claude, ProviderType::GrokOAuth) => {
            ("claude_to_grok_responses", AdapterSupport::Native)
        }
        (AppKind::Claude, ProviderType::KimiCode) => {
            ("claude_kimi_messages", AdapterSupport::Native)
        }
        (AppKind::Claude, ProviderType::QoderCosy) => {
            ("claude_to_qoder_cosy", AdapterSupport::Native)
        }

        (AppKind::Codex, ProviderType::Codex) => {
            ("codex_openai_compatible", AdapterSupport::Native)
        }
        (AppKind::Codex, ProviderType::CodexOAuth) => {
            ("codex_oauth_responses", AdapterSupport::Native)
        }
        (AppKind::Codex, ProviderType::OpenRouter) => {
            ("codex_openrouter_compatible", AdapterSupport::Native)
        }
        (AppKind::Codex, ProviderType::CursorOAuth) => {
            ("codex_cursor_agentservice", AdapterSupport::Native)
        }
        (AppKind::Codex, ProviderType::CursorApiKey) => {
            ("codex_cursor_apikey_agentservice", AdapterSupport::Native)
        }
        (AppKind::Codex, ProviderType::OllamaCloud) => {
            ("codex_ollama_openai_compatible", AdapterSupport::Native)
        }
        (
            AppKind::Codex,
            ProviderType::Claude | ProviderType::ClaudeAuth | ProviderType::ClaudeOAuth,
        ) => ("codex_to_claude_messages", AdapterSupport::Native),
        (AppKind::Codex, ProviderType::Gemini | ProviderType::GeminiCli) => {
            ("codex_to_gemini_native", AdapterSupport::Native)
        }
        (AppKind::Codex, ProviderType::GitHubCopilot) => {
            ("codex_to_copilot_chat", AdapterSupport::Native)
        }
        (AppKind::Codex, ProviderType::DeepSeekAccount) => fallback("codex_deepseek_skeleton"),
        (AppKind::Codex, ProviderType::KiroOAuth) => {
            ("codex_to_kiro_anthropic", AdapterSupport::Native)
        }
        (AppKind::Codex, ProviderType::AntigravityOAuth | ProviderType::AgyOAuth) => {
            ("codex_antigravity_gemini_native", AdapterSupport::Native)
        }
        (AppKind::Codex, ProviderType::AwsBedrock) => planned("codex_bedrock_planned"),
        (AppKind::Codex, ProviderType::Nvidia | ProviderType::DeepSeekApi) => {
            ("codex_openai_chat_compatible", AdapterSupport::Native)
        }
        (AppKind::Codex, ProviderType::GrokOAuth) => {
            ("codex_grok_responses", AdapterSupport::Native)
        }
        (AppKind::Codex, ProviderType::KimiCode) => ("codex_to_kimi_chat", AdapterSupport::Native),
        (AppKind::Codex, ProviderType::QoderCosy) => {
            ("codex_to_qoder_cosy", AdapterSupport::Native)
        }

        (AppKind::Gemini, ProviderType::Gemini) => ("gemini_api_key", AdapterSupport::Native),
        (AppKind::Gemini, ProviderType::GeminiCli) => {
            ("gemini_cli_oauth_native", AdapterSupport::Native)
        }
        (AppKind::Gemini, ProviderType::AntigravityOAuth | ProviderType::AgyOAuth) => {
            ("gemini_antigravity_native", AdapterSupport::Native)
        }
        (AppKind::Gemini, ProviderType::OpenRouter) => {
            ("gemini_openrouter_openai_chat", AdapterSupport::Native)
        }
        (
            AppKind::Gemini,
            ProviderType::Claude | ProviderType::ClaudeAuth | ProviderType::ClaudeOAuth,
        ) => ("gemini_to_claude_messages", AdapterSupport::Native),
        (AppKind::Gemini, ProviderType::Codex | ProviderType::CodexOAuth) => {
            ("gemini_to_codex_responses", AdapterSupport::Native)
        }
        (AppKind::Gemini, ProviderType::GitHubCopilot) => {
            ("gemini_to_copilot_chat", AdapterSupport::Native)
        }
        (AppKind::Gemini, ProviderType::DeepSeekAccount) => fallback("gemini_deepseek_skeleton"),
        (AppKind::Gemini, ProviderType::KiroOAuth) => fallback("gemini_kiro_skeleton"),
        (AppKind::Gemini, ProviderType::CursorOAuth | ProviderType::CursorApiKey) => {
            ("gemini_cursor_agentservice", AdapterSupport::Native)
        }
        (AppKind::Gemini, ProviderType::OllamaCloud) => {
            ("gemini_ollama_openai_chat", AdapterSupport::Native)
        }
        (AppKind::Gemini, ProviderType::AwsBedrock) => planned("gemini_bedrock_planned"),
        (AppKind::Gemini, ProviderType::Nvidia) => {
            ("gemini_nvidia_openai_chat", AdapterSupport::Native)
        }
        (AppKind::Gemini, ProviderType::DeepSeekApi) => {
            ("gemini_deepseek_openai_chat", AdapterSupport::Native)
        }
        (AppKind::Gemini, ProviderType::GrokOAuth) => {
            ("gemini_to_grok_responses", AdapterSupport::Native)
        }
        (AppKind::Gemini, ProviderType::KimiCode) => {
            ("gemini_to_kimi_chat", AdapterSupport::Native)
        }
        (AppKind::Gemini, ProviderType::QoderCosy) => {
            ("gemini_to_qoder_cosy", AdapterSupport::Native)
        }
    };

    AdapterProfile { adapter, support }
}

fn fallback(adapter: &'static str) -> (&'static str, AdapterSupport) {
    (adapter, AdapterSupport::GenericFallback)
}

fn planned(adapter: &'static str) -> (&'static str, AdapterSupport) {
    (adapter, AdapterSupport::Planned)
}

fn native_format(app: AppKind) -> &'static str {
    match app {
        AppKind::Claude => "anthropic_messages",
        AppKind::Codex => "openai_responses_or_chat",
        AppKind::Gemini => "gemini_generate_content",
    }
}

fn requires_transform(app: AppKind, provider_type: ProviderType) -> bool {
    match app {
        AppKind::Claude => matches!(
            provider_type,
            ProviderType::Codex
                | ProviderType::CodexOAuth
                | ProviderType::Gemini
                | ProviderType::GeminiCli
                | ProviderType::GitHubCopilot
                | ProviderType::KiroOAuth
                | ProviderType::DeepSeekAccount
                | ProviderType::CursorOAuth
                | ProviderType::CursorApiKey
                | ProviderType::AntigravityOAuth
                | ProviderType::AgyOAuth
                | ProviderType::OllamaCloud
                | ProviderType::Nvidia
                | ProviderType::GrokOAuth
        ),
        AppKind::Codex => matches!(
            provider_type,
            ProviderType::CodexOAuth
                | ProviderType::Claude
                | ProviderType::ClaudeAuth
                | ProviderType::ClaudeOAuth
                | ProviderType::Gemini
                | ProviderType::GeminiCli
                | ProviderType::GitHubCopilot
                | ProviderType::CursorOAuth
                | ProviderType::CursorApiKey
                | ProviderType::OllamaCloud
                | ProviderType::AntigravityOAuth
                | ProviderType::AgyOAuth
                | ProviderType::Nvidia
                | ProviderType::DeepSeekApi
                | ProviderType::GrokOAuth
                | ProviderType::KiroOAuth
                | ProviderType::KimiCode
        ),
        AppKind::Gemini => matches!(
            provider_type,
            ProviderType::Claude
                | ProviderType::ClaudeAuth
                | ProviderType::ClaudeOAuth
                | ProviderType::Codex
                | ProviderType::CodexOAuth
                | ProviderType::OpenRouter
                | ProviderType::GitHubCopilot
                | ProviderType::CursorOAuth
                | ProviderType::CursorApiKey
                | ProviderType::OllamaCloud
                | ProviderType::Nvidia
                | ProviderType::DeepSeekApi
                | ProviderType::GrokOAuth
                | ProviderType::KimiCode
        ),
    }
}

fn supports_stream_usage(app: AppKind, provider_type: ProviderType) -> bool {
    matches!(
        (app, provider_type),
        (
            AppKind::Claude,
            ProviderType::Claude
                | ProviderType::ClaudeAuth
                | ProviderType::ClaudeOAuth
                | ProviderType::Codex
                | ProviderType::CodexOAuth
                | ProviderType::Gemini
                | ProviderType::GeminiCli
                | ProviderType::OpenRouter
                | ProviderType::AntigravityOAuth
                | ProviderType::AgyOAuth
                | ProviderType::OllamaCloud
                | ProviderType::Nvidia
                | ProviderType::KiroOAuth
                | ProviderType::DeepSeekAccount
                | ProviderType::GitHubCopilot
                | ProviderType::GrokOAuth
                | ProviderType::KimiCode
        ) | (
            AppKind::Codex,
            ProviderType::Claude
                | ProviderType::ClaudeAuth
                | ProviderType::ClaudeOAuth
                | ProviderType::Codex
                | ProviderType::CodexOAuth
                | ProviderType::Gemini
                | ProviderType::GeminiCli
                | ProviderType::OpenRouter
                | ProviderType::OllamaCloud
                | ProviderType::AntigravityOAuth
                | ProviderType::AgyOAuth
                | ProviderType::Nvidia
                | ProviderType::DeepSeekApi
                | ProviderType::GrokOAuth
                | ProviderType::KiroOAuth
                | ProviderType::GitHubCopilot
                | ProviderType::KimiCode
        ) | (
            AppKind::Gemini,
            ProviderType::Gemini
                | ProviderType::GeminiCli
                | ProviderType::Claude
                | ProviderType::ClaudeAuth
                | ProviderType::ClaudeOAuth
                | ProviderType::Codex
                | ProviderType::CodexOAuth
                | ProviderType::AntigravityOAuth
                | ProviderType::AgyOAuth
                | ProviderType::OpenRouter
                | ProviderType::OllamaCloud
                | ProviderType::Nvidia
                | ProviderType::DeepSeekApi
                | ProviderType::GrokOAuth
                | ProviderType::GitHubCopilot
                | ProviderType::KimiCode
        )
    )
}

fn supports_model_list(app: AppKind, provider_type: ProviderType) -> bool {
    matches!(
        (app, provider_type),
        (
            AppKind::Claude,
            ProviderType::Claude
                | ProviderType::OpenRouter
                | ProviderType::OllamaCloud
                | ProviderType::Nvidia
                | ProviderType::DeepSeekApi
                | ProviderType::GrokOAuth
                | ProviderType::KiroOAuth
                | ProviderType::GitHubCopilot
        ) | (
            AppKind::Codex,
            ProviderType::Codex
                | ProviderType::OpenRouter
                | ProviderType::OllamaCloud
                | ProviderType::Nvidia
                | ProviderType::DeepSeekApi
                | ProviderType::GrokOAuth
                | ProviderType::KiroOAuth
                | ProviderType::GitHubCopilot
        ) | (
            AppKind::Gemini,
            ProviderType::Gemini
                | ProviderType::OpenRouter
                | ProviderType::OllamaCloud
                | ProviderType::Nvidia
                | ProviderType::DeepSeekApi
                | ProviderType::GrokOAuth
                | ProviderType::GitHubCopilot
        )
    )
}

fn base_url(app: AppKind, stored: &StoredProvider) -> Result<String, ProxyError> {
    let provider = &stored.provider;
    let provider_type = stored.provider_type;
    let value = app_configured_base_url(provider, app).or_else(|| default_base_url(provider_type));

    value.ok_or_else(|| ProxyError::bad_request("provider base url is not configured"))
}

fn header_app_for(app: AppKind, provider_type: ProviderType) -> AppKind {
    match provider_type {
        ProviderType::Claude | ProviderType::ClaudeAuth | ProviderType::ClaudeOAuth => {
            AppKind::Claude
        }
        ProviderType::Codex
        | ProviderType::CodexOAuth
        | ProviderType::OllamaCloud
        | ProviderType::GrokOAuth => AppKind::Codex,
        ProviderType::KimiCode | ProviderType::QoderCosy => app,
        ProviderType::Gemini | ProviderType::GeminiCli => AppKind::Gemini,
        ProviderType::OpenRouter => {
            if app == AppKind::Gemini {
                AppKind::Codex
            } else {
                app
            }
        }
        ProviderType::AntigravityOAuth | ProviderType::AgyOAuth => AppKind::Gemini,
        ProviderType::CursorOAuth | ProviderType::CursorApiKey => {
            if app == AppKind::Codex {
                AppKind::Codex
            } else {
                app
            }
        }
        ProviderType::GitHubCopilot | ProviderType::DeepSeekAccount | ProviderType::KiroOAuth => {
            app
        }
        ProviderType::AwsBedrock => AppKind::Claude,
        ProviderType::Nvidia | ProviderType::DeepSeekApi => {
            if app == AppKind::Gemini {
                AppKind::Codex
            } else {
                app
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UpstreamFormat {
    AnthropicMessages,
    OpenAiResponses,
    OpenAiChat,
    GeminiNative,
}

enum ExplicitUpstreamFormat {
    Passthrough,
    Transform(UpstreamFormat),
}

fn transform_body_for_upstream(
    body: Bytes,
    stored: &StoredProvider,
    route: Option<ProxyRoute>,
) -> Result<Bytes, ProxyError> {
    let Some(upstream_format) = upstream_format_for_route(stored, route, &body) else {
        return Ok(body);
    };
    let input = serde_json::from_slice::<Value>(&body).map_err(|error| {
        ProxyError::bad_request(format!(
            "request body must be valid json for transform: {error}"
        ))
    })?;
    let output = match (stored.app, upstream_format) {
        (AppKind::Claude, UpstreamFormat::OpenAiResponses) => {
            if stored.provider_type == ProviderType::CodexOAuth {
                transforms::anthropic_to_openai_responses_with_instructions(&input)
            } else {
                transforms::anthropic_to_openai_responses(&input)
            }
        }
        (AppKind::Claude, UpstreamFormat::OpenAiChat) => {
            transforms::anthropic_to_openai_chat(&input)
        }
        (AppKind::Claude, UpstreamFormat::GeminiNative) => {
            transforms::anthropic_to_gemini_native(&input)
        }
        (AppKind::Codex, UpstreamFormat::AnthropicMessages) => {
            if input.get("messages").is_some() {
                transforms::openai_chat_to_anthropic(&input)
            } else {
                transforms::openai_responses_to_anthropic(&input)
            }
        }
        (AppKind::Codex, UpstreamFormat::OpenAiChat) => {
            if input.get("messages").is_some() {
                Ok(input)
            } else {
                transforms::openai_responses_to_chat_with_reasoning_effort(
                    &input,
                    chat_reasoning_effort_mode(stored),
                )
            }
        }
        (AppKind::Codex, UpstreamFormat::OpenAiResponses) => {
            if input.get("input").is_some() {
                Ok(input)
            } else {
                transforms::openai_chat_to_responses(&input)
            }
        }
        (AppKind::Codex, UpstreamFormat::GeminiNative) => {
            let anthropic = if input.get("messages").is_some() {
                transforms::openai_chat_to_anthropic(&input)
            } else {
                transforms::openai_responses_to_anthropic(&input)
            };
            anthropic.and_then(|value| transforms::anthropic_to_gemini_native(&value))
        }
        (AppKind::Gemini, UpstreamFormat::AnthropicMessages) => {
            transforms::gemini_native_to_anthropic(&input)
        }
        (AppKind::Gemini, UpstreamFormat::OpenAiResponses) => {
            transforms::gemini_native_to_anthropic(&input).and_then(|value| {
                if stored.provider_type == ProviderType::CodexOAuth {
                    transforms::anthropic_to_openai_responses_with_instructions(&value)
                } else {
                    transforms::anthropic_to_openai_responses(&value)
                }
            })
        }
        (AppKind::Gemini, UpstreamFormat::OpenAiChat) => {
            transforms::gemini_native_to_anthropic(&input)
                .and_then(|value| transforms::anthropic_to_openai_chat(&value))
        }
        _ => Ok(input),
    }
    .map_err(|error| ProxyError::bad_request(format!("request transform failed: {error}")))?;

    serde_json::to_vec(&output)
        .map(Bytes::from)
        .map_err(|error| {
            ProxyError::bad_request(format!("request transform encode failed: {error}"))
        })
}

fn chat_reasoning_effort_mode(stored: &StoredProvider) -> transforms::ReasoningEffortMode {
    if stored.provider_type == ProviderType::OllamaCloud {
        return transforms::ReasoningEffortMode::Ollama;
    }

    let base_url = app_configured_base_url(&stored.provider, stored.app)
        .or_else(|| default_base_url(stored.provider_type))
        .unwrap_or_default();
    if base_url.to_ascii_lowercase().contains("ollama") {
        transforms::ReasoningEffortMode::Ollama
    } else {
        transforms::ReasoningEffortMode::Passthrough
    }
}

fn transform_response_for_downstream(
    body: Bytes,
    stored: &StoredProvider,
    route: ProxyRoute,
    responses_tool_context: &transforms::ResponsesToolContext,
) -> Result<Bytes, ProxyError> {
    let Some(upstream_format) = upstream_format_for_route(stored, Some(route), &[]) else {
        return Ok(body);
    };
    let downstream_format = downstream_format_for_route(route);
    let unwrap_v1internal = is_gemini_v1internal_provider_type(stored.provider_type);
    let cross_protocol = upstream_format != downstream_format;
    if upstream_format == downstream_format && !unwrap_v1internal {
        return Ok(body);
    }
    let mut input = match serde_json::from_slice::<Value>(&body) {
        Ok(input) => input,
        Err(error) if unwrap_v1internal => {
            return Err(ProxyError::bad_gateway(format!(
                "Gemini upstream returned invalid JSON: {error}"
            )));
        }
        Err(error) if cross_protocol => {
            return Err(ProxyError::bad_gateway(format!(
                "cross-protocol upstream returned invalid JSON: {error}"
            )));
        }
        Err(_) => return Ok(body),
    };
    if unwrap_v1internal {
        input = unwrap_gemini_v1internal_value(input);
    }
    let fallback_body = if unwrap_v1internal {
        serde_json::to_vec(&input)
            .map(Bytes::from)
            .unwrap_or_else(|_| body.clone())
    } else {
        body.clone()
    };
    if looks_like_error_response(&input) {
        if cross_protocol {
            return Err(ProxyError::bad_gateway(
                "cross-protocol upstream returned an error payload",
            ));
        }
        return Ok(fallback_body);
    }
    if upstream_format == downstream_format {
        return Ok(fallback_body);
    }

    let transformed = match (upstream_format, downstream_format) {
        (UpstreamFormat::OpenAiResponses, UpstreamFormat::AnthropicMessages) => {
            transforms::openai_responses_response_to_anthropic(&input)
        }
        (UpstreamFormat::OpenAiChat, UpstreamFormat::AnthropicMessages) => {
            transforms::openai_chat_response_to_anthropic(&input)
        }
        (UpstreamFormat::GeminiNative, UpstreamFormat::AnthropicMessages) => {
            transforms::gemini_response_to_anthropic(&input)
        }
        (UpstreamFormat::AnthropicMessages, UpstreamFormat::OpenAiResponses) => {
            transforms::anthropic_response_to_openai_responses_with_tool_context(
                &input,
                responses_tool_context,
            )
        }
        (UpstreamFormat::AnthropicMessages, UpstreamFormat::OpenAiChat) => {
            transforms::anthropic_response_to_openai_chat(&input)
        }
        (UpstreamFormat::OpenAiResponses, UpstreamFormat::OpenAiChat) => {
            transforms::openai_responses_response_to_chat(&input)
        }
        (UpstreamFormat::OpenAiChat, UpstreamFormat::OpenAiResponses) => {
            transforms::openai_chat_response_to_responses_with_tool_context(
                &input,
                responses_tool_context,
            )
        }
        (UpstreamFormat::AnthropicMessages, UpstreamFormat::GeminiNative) => {
            transforms::anthropic_response_to_gemini(&input)
        }
        (UpstreamFormat::GeminiNative, UpstreamFormat::OpenAiResponses) => {
            transforms::gemini_response_to_anthropic(&input).and_then(|value| {
                transforms::anthropic_response_to_openai_responses_with_tool_context(
                    &value,
                    responses_tool_context,
                )
            })
        }
        (UpstreamFormat::GeminiNative, UpstreamFormat::OpenAiChat) => {
            transforms::gemini_response_to_anthropic(&input)
                .and_then(|value| transforms::anthropic_response_to_openai_chat(&value))
        }
        (UpstreamFormat::OpenAiResponses, UpstreamFormat::GeminiNative) => {
            transforms::openai_responses_response_to_anthropic(&input)
                .and_then(|value| transforms::anthropic_response_to_gemini(&value))
        }
        (UpstreamFormat::OpenAiChat, UpstreamFormat::GeminiNative) => {
            transforms::openai_chat_response_to_anthropic(&input)
                .and_then(|value| transforms::anthropic_response_to_gemini(&value))
        }
        _ => Ok(input),
    };

    match transformed.and_then(|value| {
        serde_json::to_vec(&value)
            .map_err(|error| transforms::TransformError::new(error.to_string()))
    }) {
        Ok(bytes) => Ok(Bytes::from(bytes)),
        Err(error) => Err(ProxyError::bad_gateway(format!(
            "cross-protocol response bridge failed: {error}"
        ))),
    }
}

fn transform_stream_event_for_downstream(
    chunk: Bytes,
    stored: &StoredProvider,
    route: ProxyRoute,
    responses_tool_context: &transforms::ResponsesToolContext,
) -> Bytes {
    let Some(upstream_format) = upstream_format_for_route(stored, Some(route), &[]) else {
        return chunk;
    };
    let downstream_format = downstream_format_for_route(route);
    let unwrap_v1internal = is_gemini_v1internal_provider_type(stored.provider_type);
    if upstream_format == downstream_format && !unwrap_v1internal {
        return chunk;
    }
    let Ok(text) = std::str::from_utf8(&chunk) else {
        return chunk;
    };

    let mut output = String::new();
    let mut converted = false;
    for line in text.lines() {
        let Some(payload) = stream_json_payload(line) else {
            continue;
        };
        if payload == "[DONE]" {
            output.push_str("data: [DONE]\n\n");
            converted = true;
            continue;
        }
        let Ok(mut value) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        if unwrap_v1internal {
            value = unwrap_gemini_v1internal_value(value);
        }
        let frames = if upstream_format == downstream_format {
            vec![transforms::StreamFrame::json(value)]
        } else {
            transform_stream_value(
                upstream_format,
                downstream_format,
                &value,
                responses_tool_context,
            )
        };
        if frames.is_empty() {
            continue;
        }
        converted = true;
        output.push_str(&encode_stream_frames(&frames));
    }

    if converted {
        Bytes::from(output)
    } else {
        chunk
    }
}

pub(super) fn transform_stream_value(
    upstream_format: UpstreamFormat,
    downstream_format: UpstreamFormat,
    value: &Value,
    responses_tool_context: &transforms::ResponsesToolContext,
) -> Vec<transforms::StreamFrame> {
    match (upstream_format, downstream_format) {
        (UpstreamFormat::OpenAiResponses, UpstreamFormat::AnthropicMessages) => {
            transforms::openai_responses_stream_to_anthropic(value)
        }
        (UpstreamFormat::OpenAiChat, UpstreamFormat::AnthropicMessages) => {
            transforms::openai_chat_stream_to_anthropic(value)
        }
        (UpstreamFormat::GeminiNative, UpstreamFormat::AnthropicMessages) => {
            transforms::gemini_stream_to_anthropic(value)
        }
        (UpstreamFormat::AnthropicMessages, UpstreamFormat::OpenAiResponses) => {
            transforms::anthropic_stream_to_openai_responses(value)
        }
        (UpstreamFormat::AnthropicMessages, UpstreamFormat::OpenAiChat) => {
            transforms::anthropic_stream_to_openai_chat(value)
        }
        (UpstreamFormat::OpenAiResponses, UpstreamFormat::OpenAiChat) => {
            transforms::openai_responses_stream_to_chat(value)
        }
        (UpstreamFormat::OpenAiChat, UpstreamFormat::OpenAiResponses) => {
            transforms::openai_chat_stream_to_responses_with_tool_context(
                value,
                responses_tool_context,
            )
        }
        (UpstreamFormat::AnthropicMessages, UpstreamFormat::GeminiNative) => {
            transforms::anthropic_stream_to_gemini(value)
        }
        (UpstreamFormat::OpenAiResponses, UpstreamFormat::GeminiNative) => {
            transforms::openai_responses_stream_to_gemini(value)
        }
        (UpstreamFormat::OpenAiChat, UpstreamFormat::GeminiNative) => {
            transforms::openai_chat_stream_to_gemini(value)
        }
        (UpstreamFormat::GeminiNative, UpstreamFormat::OpenAiResponses) => {
            transforms::gemini_stream_to_openai_responses(value)
        }
        (UpstreamFormat::GeminiNative, UpstreamFormat::OpenAiChat) => {
            transforms::gemini_stream_to_openai_chat(value)
        }
        _ => Vec::new(),
    }
}

fn transform_stream_frames(
    frames: Vec<transforms::StreamFrame>,
    next: fn(&Value) -> Vec<transforms::StreamFrame>,
) -> Vec<transforms::StreamFrame> {
    frames
        .into_iter()
        .flat_map(|frame| match frame.payload {
            transforms::StreamPayload::Json(value) => next(&value),
            transforms::StreamPayload::Done => vec![transforms::StreamFrame::done()],
        })
        .collect()
}

fn stream_json_payload(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.is_empty() || line.starts_with("event:") {
        return None;
    }
    if let Some(payload) = line.strip_prefix("data:") {
        return Some(payload.trim());
    }
    line.starts_with('{').then_some(line)
}

pub(super) fn encode_stream_frames(frames: &[transforms::StreamFrame]) -> String {
    let mut output = String::new();
    for frame in frames {
        if let Some(event) = frame.event {
            output.push_str("event: ");
            output.push_str(event);
            output.push('\n');
        }
        match &frame.payload {
            transforms::StreamPayload::Json(value) => {
                if let Some(event) = super::responses_wire::encode_sse_event(value) {
                    output.push_str(&event);
                    continue;
                }
                if let Ok(data) = serde_json::to_string(value) {
                    output.push_str("data: ");
                    output.push_str(&data);
                    output.push_str("\n\n");
                }
            }
            transforms::StreamPayload::Done => output.push_str("data: [DONE]\n\n"),
        }
    }
    output
}

fn looks_like_error_response(value: &Value) -> bool {
    google_embedded_error(value).is_some()
        || ["errors", "error_message", "errorMessage"]
            .into_iter()
            .filter_map(|key| value.get(key))
            .any(super::response_semantics::error_value_is_substantive)
}

pub(super) fn upstream_format_for_route(
    stored: &StoredProvider,
    route: Option<ProxyRoute>,
    body: &[u8],
) -> Option<UpstreamFormat> {
    if is_gemini_v1internal_provider_type(stored.provider_type) {
        return Some(UpstreamFormat::GeminiNative);
    }
    if stored.provider_type == ProviderType::GrokOAuth {
        if matches!(route, Some(ProxyRoute::CodexChatCompletions)) {
            return Some(UpstreamFormat::OpenAiResponses);
        }
        if matches!(
            route,
            Some(ProxyRoute::CodexResponses | ProxyRoute::CodexResponsesCompact)
        ) {
            return Some(UpstreamFormat::OpenAiResponses);
        }
    }
    upstream_format_for(stored, body)
}

fn upstream_format_for(stored: &StoredProvider, body: &[u8]) -> Option<UpstreamFormat> {
    match explicit_upstream_format(stored) {
        Some(ExplicitUpstreamFormat::Passthrough) => return None,
        Some(ExplicitUpstreamFormat::Transform(format)) => return Some(format),
        None => {}
    }

    match stored.app {
        AppKind::Claude => match stored.provider_type {
            ProviderType::Codex | ProviderType::CodexOAuth => Some(UpstreamFormat::OpenAiResponses),
            ProviderType::KiroOAuth => Some(UpstreamFormat::AnthropicMessages),
            ProviderType::GitHubCopilot => Some(UpstreamFormat::OpenAiChat),
            ProviderType::CursorOAuth | ProviderType::CursorApiKey => {
                Some(UpstreamFormat::OpenAiChat)
            }
            ProviderType::OllamaCloud => Some(UpstreamFormat::OpenAiChat),
            ProviderType::Nvidia => Some(UpstreamFormat::OpenAiChat),
            ProviderType::GrokOAuth => Some(UpstreamFormat::OpenAiResponses),
            ProviderType::KimiCode => Some(UpstreamFormat::AnthropicMessages),
            ProviderType::QoderCosy => Some(UpstreamFormat::OpenAiChat),
            ProviderType::Gemini | ProviderType::GeminiCli => Some(UpstreamFormat::GeminiNative),
            ProviderType::AntigravityOAuth | ProviderType::AgyOAuth => {
                Some(UpstreamFormat::GeminiNative)
            }
            _ => None,
        },
        AppKind::Codex => match stored.provider_type {
            ProviderType::CodexOAuth => Some(UpstreamFormat::OpenAiResponses),
            ProviderType::GitHubCopilot => Some(UpstreamFormat::OpenAiChat),
            ProviderType::OllamaCloud => Some(UpstreamFormat::OpenAiChat),
            ProviderType::CursorOAuth | ProviderType::CursorApiKey => {
                Some(UpstreamFormat::OpenAiChat)
            }
            ProviderType::Nvidia | ProviderType::DeepSeekApi => Some(UpstreamFormat::OpenAiChat),
            ProviderType::GrokOAuth => Some(UpstreamFormat::OpenAiResponses),
            ProviderType::KimiCode | ProviderType::QoderCosy => Some(UpstreamFormat::OpenAiChat),
            ProviderType::KiroOAuth => Some(UpstreamFormat::AnthropicMessages),
            ProviderType::Claude | ProviderType::ClaudeAuth | ProviderType::ClaudeOAuth => {
                Some(UpstreamFormat::AnthropicMessages)
            }
            ProviderType::Gemini | ProviderType::GeminiCli => Some(UpstreamFormat::GeminiNative),
            ProviderType::AntigravityOAuth | ProviderType::AgyOAuth => {
                Some(UpstreamFormat::GeminiNative)
            }
            _ => None,
        },
        AppKind::Gemini => match stored.provider_type {
            ProviderType::Claude | ProviderType::ClaudeAuth | ProviderType::ClaudeOAuth => {
                Some(UpstreamFormat::AnthropicMessages)
            }
            ProviderType::OpenRouter => Some(UpstreamFormat::OpenAiChat),
            ProviderType::GitHubCopilot => Some(UpstreamFormat::OpenAiChat),
            ProviderType::CursorOAuth | ProviderType::CursorApiKey | ProviderType::OllamaCloud => {
                Some(UpstreamFormat::OpenAiChat)
            }
            ProviderType::Nvidia | ProviderType::DeepSeekApi => Some(UpstreamFormat::OpenAiChat),
            ProviderType::GrokOAuth => Some(UpstreamFormat::OpenAiResponses),
            ProviderType::KimiCode | ProviderType::QoderCosy => Some(UpstreamFormat::OpenAiChat),
            ProviderType::Codex | ProviderType::CodexOAuth => Some(UpstreamFormat::OpenAiResponses),
            ProviderType::GeminiCli | ProviderType::AntigravityOAuth | ProviderType::AgyOAuth => {
                Some(UpstreamFormat::GeminiNative)
            }
            _ => {
                let _ = body;
                None
            }
        },
    }
}

fn explicit_upstream_format(stored: &StoredProvider) -> Option<ExplicitUpstreamFormat> {
    match configured_api_format(&stored.provider) {
        Some("anthropic") if stored.app == AppKind::Claude => {
            Some(ExplicitUpstreamFormat::Passthrough)
        }
        Some("anthropic") => Some(ExplicitUpstreamFormat::Transform(
            UpstreamFormat::AnthropicMessages,
        )),
        Some("openai_responses") => Some(ExplicitUpstreamFormat::Transform(
            UpstreamFormat::OpenAiResponses,
        )),
        Some("openai_chat") => Some(ExplicitUpstreamFormat::Transform(
            UpstreamFormat::OpenAiChat,
        )),
        Some("gemini_native") if stored.app == AppKind::Gemini => {
            Some(ExplicitUpstreamFormat::Passthrough)
        }
        Some("gemini_native") => Some(ExplicitUpstreamFormat::Transform(
            UpstreamFormat::GeminiNative,
        )),
        _ => None,
    }
}

pub(super) fn downstream_format_for_route(route: ProxyRoute) -> UpstreamFormat {
    match route {
        ProxyRoute::ClaudeMessages | ProxyRoute::ClaudeCountTokens => {
            UpstreamFormat::AnthropicMessages
        }
        ProxyRoute::CodexResponses | ProxyRoute::CodexResponsesCompact => {
            UpstreamFormat::OpenAiResponses
        }
        ProxyRoute::CodexChatCompletions => UpstreamFormat::OpenAiChat,
        ProxyRoute::Gemini => UpstreamFormat::GeminiNative,
    }
}

fn downstream_format_for_request(
    stored: &StoredProvider,
    route: Option<ProxyRoute>,
) -> UpstreamFormat {
    route
        .map(downstream_format_for_route)
        .unwrap_or_else(|| match stored.app {
            AppKind::Claude => UpstreamFormat::AnthropicMessages,
            AppKind::Codex => UpstreamFormat::OpenAiResponses,
            AppKind::Gemini => UpstreamFormat::GeminiNative,
        })
}

fn maybe_inject_downstream_prompt_cache(
    body: Bytes,
    stored: &StoredProvider,
    route: Option<ProxyRoute>,
    config: &CacheInjectionConfig,
) -> Result<Bytes, ProxyError> {
    if downstream_format_for_request(stored, route) != UpstreamFormat::AnthropicMessages {
        return Ok(body);
    }
    inject_prompt_cache_body(body, config)
}

fn maybe_inject_upstream_prompt_cache(
    body: Bytes,
    stored: &StoredProvider,
    route: Option<ProxyRoute>,
    config: &CacheInjectionConfig,
) -> Result<Bytes, ProxyError> {
    if !is_anthropic_request_body(stored, route, &body) {
        return Ok(body);
    }
    inject_prompt_cache_body(body, config)
}

fn maybe_apply_thinking_pipeline(
    body: Bytes,
    stored: &StoredProvider,
    route: Option<ProxyRoute>,
    config: &ThinkingPipelineConfig,
) -> Result<Bytes, ProxyError> {
    if !is_anthropic_request_body(stored, route, &body) {
        return Ok(body);
    }
    apply_thinking_pipeline_body(body, config)
}

fn maybe_apply_request_governance(
    body: Bytes,
    stored: &StoredProvider,
    config: &RequestGovernanceConfig,
) -> Result<Bytes, ProxyError> {
    if !config.is_enabled() {
        return Ok(body);
    }
    let mut value = serde_json::from_slice::<Value>(&body).map_err(|error| {
        ProxyError::bad_request(format!(
            "request body must be valid json for request governance: {error}"
        ))
    })?;
    govern_request_body(&mut value, &stored.provider.settings_config, config);
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|error| {
            ProxyError::bad_request(format!("request governance encode failed: {error}"))
        })
}

fn is_anthropic_request_body(
    stored: &StoredProvider,
    route: Option<ProxyRoute>,
    body: &[u8],
) -> bool {
    match upstream_format_for(stored, body) {
        Some(UpstreamFormat::AnthropicMessages) => true,
        Some(_) => false,
        None => downstream_format_for_request(stored, route) == UpstreamFormat::AnthropicMessages,
    }
}

fn inject_prompt_cache_body(
    body: Bytes,
    config: &CacheInjectionConfig,
) -> Result<Bytes, ProxyError> {
    if !config.enabled {
        return Ok(body);
    }
    let mut value = serde_json::from_slice::<Value>(&body).map_err(|error| {
        ProxyError::bad_request(format!(
            "request body must be valid json for cache injection: {error}"
        ))
    })?;
    inject_prompt_cache(&mut value, config);
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|error| {
            ProxyError::bad_request(format!("request cache injection encode failed: {error}"))
        })
}

fn apply_thinking_pipeline_body(
    body: Bytes,
    config: &ThinkingPipelineConfig,
) -> Result<Bytes, ProxyError> {
    if !config.is_enabled() {
        return Ok(body);
    }
    let mut value = serde_json::from_slice::<Value>(&body).map_err(|error| {
        ProxyError::bad_request(format!(
            "request body must be valid json for thinking pipeline: {error}"
        ))
    })?;
    apply_thinking_pipeline(&mut value, config);
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|error| {
            ProxyError::bad_request(format!("request thinking pipeline encode failed: {error}"))
        })
}

fn upstream_path(
    upstream_format: UpstreamFormat,
    route: ProxyRoute,
    gemini_path: Option<String>,
    request: &AdapterRequest,
) -> String {
    match upstream_format {
        UpstreamFormat::AnthropicMessages if route == ProxyRoute::ClaudeCountTokens => {
            "/v1/messages/count_tokens".to_string()
        }
        UpstreamFormat::AnthropicMessages => "/v1/messages".to_string(),
        UpstreamFormat::OpenAiResponses if route == ProxyRoute::CodexResponsesCompact => {
            "/v1/responses/compact".to_string()
        }
        UpstreamFormat::OpenAiResponses => "/v1/responses".to_string(),
        UpstreamFormat::OpenAiChat => "/v1/chat/completions".to_string(),
        UpstreamFormat::GeminiNative => {
            if route == ProxyRoute::Gemini {
                return route.path(gemini_path);
            }
            let model = request
                .actual_model
                .as_deref()
                .or(request.model.as_deref())
                .unwrap_or("gemini-pro");
            let method = if request.upstream_stream_requested {
                "streamGenerateContent"
            } else {
                "generateContent"
            };
            format!("/v1beta/models/{model}:{method}")
        }
    }
}

fn kimi_upstream_path(upstream_format: UpstreamFormat, route: ProxyRoute) -> String {
    match (upstream_format, route) {
        (UpstreamFormat::AnthropicMessages, ProxyRoute::ClaudeCountTokens) => {
            "/v1/messages/count_tokens?beta=true".to_string()
        }
        (UpstreamFormat::AnthropicMessages, _) => "/v1/messages?beta=true".to_string(),
        (UpstreamFormat::OpenAiChat, _) => "/v1/chat/completions".to_string(),
        _ => unreachable!("Kimi driver selected an unsupported upstream format"),
    }
}

fn upstream_path_for_provider(
    stored: &StoredProvider,
    upstream_format: UpstreamFormat,
    route: ProxyRoute,
    gemini_path: Option<String>,
    request: &AdapterRequest,
) -> String {
    if is_gemini_v1internal_provider_type(stored.provider_type)
        && upstream_format == UpstreamFormat::GeminiNative
    {
        return gemini_v1internal_path();
    }
    if stored.provider_type == ProviderType::GitHubCopilot
        && upstream_format == UpstreamFormat::OpenAiChat
    {
        return "/chat/completions".to_string();
    }
    upstream_path(upstream_format, route, gemini_path, request)
}

fn base_url_for_upstream(
    upstream_format: UpstreamFormat,
    stored: &StoredProvider,
) -> Result<String, ProxyError> {
    let provider = &stored.provider;
    let provider_type = stored.provider_type;
    let value = app_configured_base_url(provider, stored.app)
        .or_else(|| match upstream_format {
            UpstreamFormat::AnthropicMessages => {
                setting(provider, &["ANTHROPIC_BASE_URL", "BASE_URL"])
            }
            UpstreamFormat::OpenAiResponses | UpstreamFormat::OpenAiChat => setting(
                provider,
                &["OPENAI_BASE_URL", "CODEX_BASE_URL", "BASE_URL", "base_url"],
            )
            .or_else(|| codex_config_base_url(provider)),
            UpstreamFormat::GeminiNative => setting(
                provider,
                &["GOOGLE_GEMINI_BASE_URL", "GEMINI_BASE_URL", "BASE_URL"],
            ),
        })
        .or_else(|| default_base_url(provider_type));

    value.ok_or_else(|| ProxyError::bad_request("provider base url is not configured"))
}

/// Applies the OpenAI Responses/Chat endpoint contract to origin-only URLs.
fn is_origin_only_url(value: &str) -> bool {
    let trimmed = value.trim_end_matches('/');
    match trimmed.split_once("://") {
        Some((_scheme, rest)) => !rest.contains('/'),
        None => !trimmed.contains('/'),
    }
}

fn uses_codex_openai_upstream_join(path: &str) -> bool {
    let path_trimmed = path.trim_start_matches('/');
    path_trimmed == "responses"
        || path_trimmed == "chat/completions"
        || path_trimmed.starts_with("v1/responses")
        || path_trimmed.starts_with("v1/chat/completions")
}

fn join_codex_openai_upstream_url(base_trimmed: &str, path: &str) -> String {
    let path_trimmed = path.trim_start_matches('/');
    let endpoint = path_trimmed.strip_prefix("v1/").unwrap_or(path_trimmed);
    let already_has_v1 = base_trimmed.ends_with("/v1");
    let origin_only = is_origin_only_url(base_trimmed);

    let mut url = if already_has_v1 {
        format!("{base_trimmed}/{endpoint}")
    } else if origin_only {
        format!("{base_trimmed}/v1/{endpoint}")
    } else {
        format!("{base_trimmed}/{endpoint}")
    };

    while url.contains("/v1/v1") {
        url = url.replace("/v1/v1", "/v1");
    }
    url
}

fn should_use_codex_custom_prefix_join(base_trimmed: &str) -> bool {
    let lower = base_trimmed.to_ascii_lowercase();
    lower.contains("chatgpt.com/backend-api/codex") || lower.ends_with("/openai")
}

fn join_upstream_url(base_url: &str, path: &str) -> String {
    if uses_codex_openai_upstream_join(path)
        && should_use_codex_custom_prefix_join(base_url.trim_end_matches('/'))
    {
        return join_codex_openai_upstream_url(base_url.trim_end_matches('/'), path);
    }
    join_url(&base_without_duplicate_version(base_url, path), path)
}

fn base_without_duplicate_version(base_url: &str, path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    for version in ["/v1", "/v1beta"] {
        if path.starts_with(&format!("{version}/")) || path == version {
            if let Some(stripped) = base.strip_suffix(version) {
                return stripped.to_string();
            }
        }
    }
    base.to_string()
}

fn app_configured_base_url(
    provider: &crate::domain::providers::model::Provider,
    app: AppKind,
) -> Option<String> {
    match app {
        AppKind::Claude => setting(provider, &["ANTHROPIC_BASE_URL", "BASE_URL"]),
        AppKind::Codex => setting(
            provider,
            &["OPENAI_BASE_URL", "CODEX_BASE_URL", "BASE_URL", "base_url"],
        )
        .or_else(|| codex_config_base_url(provider)),
        AppKind::Gemini => setting(
            provider,
            &["GOOGLE_GEMINI_BASE_URL", "GEMINI_BASE_URL", "BASE_URL"],
        ),
    }
}

fn configured_api_format(provider: &crate::domain::providers::model::Provider) -> Option<&str> {
    provider
        .meta
        .as_ref()
        .and_then(|meta| meta.api_format.as_deref())
        .or_else(|| {
            provider
                .settings_config
                .get("apiFormat")
                .and_then(Value::as_str)
        })
        .or_else(|| {
            provider
                .settings_config
                .get("api_format")
                .and_then(Value::as_str)
        })
}

fn default_base_url(provider_type: ProviderType) -> Option<String> {
    match provider_type {
        ProviderType::Claude | ProviderType::ClaudeOAuth => {
            Some("https://api.anthropic.com".to_string())
        }
        ProviderType::Codex => Some("https://api.openai.com".to_string()),
        ProviderType::CodexOAuth => Some("https://chatgpt.com/backend-api/codex".to_string()),
        ProviderType::Gemini => Some("https://generativelanguage.googleapis.com".to_string()),
        ProviderType::GeminiCli => Some("https://cloudcode-pa.googleapis.com".to_string()),
        ProviderType::AntigravityOAuth | ProviderType::AgyOAuth => {
            Some("https://daily-cloudcode-pa.googleapis.com".to_string())
        }
        ProviderType::OpenRouter => Some("https://openrouter.ai/api".to_string()),
        ProviderType::GitHubCopilot => Some("https://api.githubcopilot.com".to_string()),
        ProviderType::CursorOAuth => Some("https://api2.cursor.sh".to_string()),
        ProviderType::CursorApiKey => Some("https://api.cursor.com".to_string()),
        ProviderType::OllamaCloud => Some("https://ollama.com".to_string()),
        ProviderType::Nvidia => Some("https://integrate.api.nvidia.com".to_string()),
        ProviderType::DeepSeekApi => Some("https://api.deepseek.com".to_string()),
        ProviderType::GrokOAuth => Some(super::grok::default_base_url().to_string()),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BedrockSigV4RequestPlan {
    endpoint: String,
    region: String,
    service: &'static str,
    host: String,
    canonical_uri: String,
    amz_date: String,
    body: Value,
    credential_scope: String,
    signed_headers: String,
    payload_hash: String,
    canonical_request_hash: String,
    authorization_header: String,
    redacted_authorization: String,
    redacted_session_token: Option<String>,
    signing_status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BedrockSignedRequestParts {
    endpoint: String,
    body: Vec<u8>,
    headers: Vec<(&'static str, String)>,
    plan: BedrockSigV4RequestPlan,
}

fn apply_bedrock_forward_contract(
    stored: &StoredProvider,
    request: &mut AdapterRequest,
) -> Result<(), ProxyError> {
    if stored.app != AppKind::Claude || stored.provider_type != ProviderType::AwsBedrock {
        return Ok(());
    }
    if stored.resource.profile_id.is_some() {
        return Ok(());
    }
    let (date_yyyymmdd, amz_date) = sigv4_dates_now();
    let signed = bedrock_sigv4_signed_request_parts(stored, request, &date_yyyymmdd, &amz_date)?;
    request.body = Bytes::from(signed.body);
    request.upstream_endpoint = Some(signed.endpoint);
    request.upstream_headers = signed.headers;
    Ok(())
}

pub(super) fn finalize_runtime_request(
    plan: &ProviderRuntimePlan,
    stored: &StoredProvider,
    request: &mut AdapterRequest,
) -> Result<(), ProxyError> {
    normalize_coding_plan_request(plan, request)?;
    if stored.provider_type == ProviderType::KimiCode {
        super::kimi::finalize_request(plan, request)?;
    }
    if plan.driver_id.as_str() == "http.bedrock_bearer" {
        apply_bedrock_bearer_forward_contract(plan, request)?;
    }
    if is_gemini_v1internal_provider_type(stored.provider_type) {
        let count_tokens = request.is_gemini_count_tokens();
        request.stream_requested &= !count_tokens;
        request.upstream_stream_requested = !count_tokens;
    }
    Ok(())
}

fn normalize_coding_plan_request(
    plan: &ProviderRuntimePlan,
    request: &mut AdapterRequest,
) -> Result<(), ProxyError> {
    if plan.coding_plan.is_none()
        || plan.upstream_protocol != UpstreamProtocol::AnthropicMessages
        || !matches!(
            plan.profile_id.as_str(),
            "claude.minimax_cn" | "claude.minimax_global"
        )
    {
        return Ok(());
    }

    let model = request
        .actual_model
        .as_deref()
        .or(request.model.as_deref())
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if !model.starts_with("minimax-m") {
        return Ok(());
    }

    let Ok(mut body) = serde_json::from_slice::<Value>(&request.body) else {
        return Ok(());
    };
    if body.pointer("/thinking/type").and_then(Value::as_str) != Some("enabled") {
        return Ok(());
    }
    let Some(thinking) = body.get_mut("thinking").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    thinking.insert("type".to_string(), Value::String("adaptive".to_string()));
    request.body = serde_json::to_vec(&body)
        .map(Bytes::from)
        .map_err(|error| {
            ProxyError::bad_request(format!(
                "MiniMax coding-plan thinking normalization encode failed: {error}"
            ))
        })?;
    Ok(())
}

pub(super) fn finalize_runtime_protocol_auth(
    plan: &ProviderRuntimePlan,
    stored: &StoredProvider,
    accounts: &AccountStore,
    request: &mut AdapterRequest,
    endpoint: &mut String,
    headers: &mut Vec<(String, String)>,
) -> Result<(), ProxyError> {
    if stored.provider_type == ProviderType::KimiCode {
        super::kimi::finalize_account_identity(plan, accounts, headers)?;
    }
    if is_gemini_v1internal_provider_type(stored.provider_type) {
        apply_gemini_v1internal_contract(plan, stored, accounts, request, endpoint)?;
    }
    if plan.driver_id.as_str() != "aws.bedrock_sigv4" {
        return Ok(());
    }
    let (date_yyyymmdd, amz_date) = sigv4_dates_now();
    let signed = bedrock_sigv4_signed_request_parts(stored, request, &date_yyyymmdd, &amz_date)?;
    request.body = Bytes::from(signed.body);
    *endpoint = signed.endpoint;
    headers.retain(|(name, _)| {
        !matches!(
            name.to_ascii_lowercase().as_str(),
            "authorization"
                | "content-type"
                | "host"
                | "x-amz-content-sha256"
                | "x-amz-date"
                | "x-amz-security-token"
        )
    });
    headers.extend(
        signed
            .headers
            .into_iter()
            .map(|(name, value)| (name.to_string(), value)),
    );
    Ok(())
}

pub(super) fn is_gemini_v1internal_provider_type(provider_type: ProviderType) -> bool {
    matches!(
        provider_type,
        ProviderType::GeminiCli | ProviderType::AntigravityOAuth | ProviderType::AgyOAuth
    )
}

fn has_antigravity_v1internal_identity(provider_type: ProviderType) -> bool {
    matches!(
        provider_type,
        ProviderType::AntigravityOAuth | ProviderType::AgyOAuth
    )
}

pub(super) fn unwrap_gemini_v1internal_value(value: Value) -> Value {
    match value {
        Value::Object(mut wrapper) => match wrapper.remove("response") {
            Some(Value::Object(mut response)) => {
                for (key, value) in wrapper {
                    response.entry(key).or_insert(value);
                }
                Value::Object(response)
            }
            Some(response) => response,
            None => Value::Object(wrapper),
        },
        value => value,
    }
}

pub(super) fn google_embedded_error(value: &Value) -> Option<ProxyError> {
    let error = value
        .get("error")
        .filter(|error| super::response_semantics::error_value_is_substantive(error))
        .or_else(|| {
            value
                .pointer("/response/error")
                .filter(|error| super::response_semantics::error_value_is_substantive(error))
        })?;
    let code = error.get("code").and_then(|value| match value {
        Value::Number(value) => Some(value.to_string()),
        Value::String(value) if !value.trim().is_empty() => Some(value.trim().to_string()),
        _ => None,
    });
    let status = error
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("upstream returned an embedded Google error");
    let mut identity = Vec::new();
    if let Some(code) = code {
        identity.push(format!("code={code}"));
    }
    if let Some(status) = status {
        identity.push(format!("status={status}"));
    }
    let identity = (!identity.is_empty()).then(|| format!(" ({})", identity.join(", ")));
    Some(ProxyError::bad_gateway(format!(
        "Google upstream embedded error{}: {message}",
        identity.as_deref().unwrap_or_default()
    )))
}

fn prepare_gemini_v1internal_route_request(
    request: &mut AdapterRequest,
    stored: &StoredProvider,
    route: ProxyRoute,
    gemini_path: Option<&str>,
) -> Result<(), ProxyError> {
    if !is_gemini_v1internal_provider_type(stored.provider_type) || route != ProxyRoute::Gemini {
        return Ok(());
    }
    if let Some(path) = gemini_path {
        let action = path
            .rsplit_once(':')
            .map(|(_, action)| action.split('?').next().unwrap_or(action))
            .unwrap_or_default();
        request.gemini_action = Some(match action {
            "generateContent" => GeminiRouteAction::GenerateContent,
            "streamGenerateContent" => GeminiRouteAction::StreamGenerateContent,
            "countTokens" => GeminiRouteAction::CountTokens,
            _ => {
                return Err(ProxyError::bad_request(format!(
                    "Gemini v1internal does not support action {action}"
                )))
            }
        });
    }
    if let Some(model) = gemini_model_from_path(gemini_path) {
        request.requested_model.get_or_insert_with(|| model.clone());
        request.model.get_or_insert(model);
    }
    Ok(())
}

fn apply_gemini_v1internal_contract(
    plan: &ProviderRuntimePlan,
    stored: &StoredProvider,
    accounts: &AccountStore,
    request: &mut AdapterRequest,
    endpoint: &mut String,
) -> Result<(), ProxyError> {
    let expected_provider_type = match plan.driver_id.as_str() {
        "oauth.gemini_code_assist" => ProviderType::GeminiCli,
        "special.antigravity" => ProviderType::AntigravityOAuth,
        "special.agy" => ProviderType::AgyOAuth,
        _ if plan.upstream_protocol == UpstreamProtocol::Legacy => stored.provider_type,
        _ => {
            return Err(ProxyError::bad_request(format!(
                "driver {} cannot execute {} v1internal protocol",
                plan.driver_id,
                stored.provider_type.as_str()
            )))
        }
    };
    if stored.provider_type != expected_provider_type {
        return Err(ProxyError::bad_request(format!(
            "driver {} requires {} account identity, not {}",
            plan.driver_id,
            expected_provider_type.as_str(),
            stored.provider_type.as_str()
        )));
    }
    let account_id = match &plan.auth_ref {
        crate::domain::providers::runtime::RuntimeAuthRef::ManagedAccount {
            account_id,
            expected_provider_type: bound_provider_type,
            ..
        } if *bound_provider_type == expected_provider_type => account_id.as_str(),
        crate::domain::providers::runtime::RuntimeAuthRef::Legacy {
            account_id: Some(account_id),
            ..
        } => account_id.as_str(),
        _ => {
            return Err(ProxyError::bad_request(format!(
                "{} v1internal requires one fixed managed account",
                stored.provider_type.as_str()
            )))
        }
    };
    let account = accounts
        .accounts
        .iter()
        .find(|account| account.id == account_id && account.provider_type == expected_provider_type)
        .ok_or_else(|| {
            ProxyError::bad_request(format!(
                "bound {} account {account_id} is unavailable",
                expected_provider_type.as_str()
            ))
        })?;
    let mut model = gemini_v1internal_model(request)?.to_string();
    let mut inner = serde_json::from_slice::<Value>(&request.body).map_err(|error| {
        ProxyError::bad_request(format!(
            "Gemini v1internal request must be valid JSON: {error}"
        ))
    })?;
    if request.is_gemini_count_tokens() {
        sanitize_gemini_v1internal_request(&mut inner, false)?;
        request.body = serde_json::to_vec(&inner)
            .map(Bytes::from)
            .map_err(|error| {
                ProxyError::bad_request(format!(
                    "Gemini countTokens request encode failed: {error}"
                ))
            })?;
        request.stream_requested = false;
        request.upstream_stream_requested = false;
        *endpoint =
            gemini_ai_studio_count_tokens_endpoint(gemini_ai_studio_base_url(account), &model);
        return Ok(());
    }
    let project_id = crate::domain::accounts::store::gemini_v1internal_project_id(account)
        .ok_or_else(|| {
        ProxyError::bad_request(format!(
            "bound {} account {account_id} has no projectId; refresh account quota or import project metadata",
            expected_provider_type.as_str()
        ))
    })?;
    let antigravity_identity = has_antigravity_v1internal_identity(stored.provider_type);
    sanitize_gemini_v1internal_request(&mut inner, antigravity_identity)?;
    let web_search = antigravity_identity && gemini_request_has_google_search(&inner);
    if web_search && model != ANTIGRAVITY_WEB_SEARCH_FALLBACK_MODEL {
        model = ANTIGRAVITY_WEB_SEARCH_FALLBACK_MODEL.to_string();
        request.model = Some(model.clone());
        request.actual_model = Some(model.clone());
        request.actual_model_source = Some("antigravity_web_search_fallback".to_string());
    }
    let mut envelope = serde_json::Map::from_iter([
        ("project".to_string(), Value::String(project_id)),
        ("model".to_string(), Value::String(model)),
        ("request".to_string(), inner),
    ]);
    if antigravity_identity {
        envelope.insert(
            "requestId".to_string(),
            Value::String(format!("agent-{}", random_uuid_like())),
        );
        envelope.insert(
            "userAgent".to_string(),
            Value::String("antigravity".to_string()),
        );
        envelope.insert(
            "requestType".to_string(),
            Value::String(if web_search { "web_search" } else { "agent" }.to_string()),
        );
    }
    request.body = serde_json::to_vec(&Value::Object(envelope))
        .map(Bytes::from)
        .map_err(|error| {
            ProxyError::bad_request(format!("Gemini v1internal request encode failed: {error}"))
        })?;
    request.upstream_stream_requested = true;
    *endpoint = gemini_v1internal_endpoint(&plan.endpoint);
    if antigravity_identity {
        select_antigravity_inference_endpoint(endpoint, account);
    }
    Ok(())
}

const GEMINI_AI_STUDIO_BASE_URL: &str = "https://generativelanguage.googleapis.com";
const ANTIGRAVITY_PRODUCTION_BASE_URL: &str = "https://cloudcode-pa.googleapis.com";
const ANTIGRAVITY_DAILY_BASE_URL: &str = "https://daily-cloudcode-pa.googleapis.com";
const ANTIGRAVITY_WEB_SEARCH_FALLBACK_MODEL: &str = "gemini-2.5-flash";
const ANTIGRAVITY_IDENTITY_PATCH: &str = "<identity>\nYou are Antigravity, a powerful agentic AI coding assistant designed by the Google Deepmind team working on Advanced Agentic Coding.\nYou are pair programming with a USER to solve their coding task. The task may require creating a new codebase, modifying or debugging an existing codebase, or simply answering a question.\nThe USER will send you requests, which you must always prioritize addressing. Along with each USER request, we will attach additional metadata about their current state, such as what files they have open and where their cursor is.\nThis information may or may not be relevant to the coding task, it is up for you to decide.\n</identity>\n<communication_style>\n- **Proactiveness**. As an agent, you are allowed to be proactive, but only in the course of completing the user's task. For example, if the user asks you to add a new component, you can edit the code, verify build and test statuses, and take any other obvious follow-up actions, such as performing additional research. However, avoid surprising the user. For example, if the user asks HOW to approach something, you should answer their question and instead of jumping into editing a file.</communication_style>";

fn gemini_v1internal_model(request: &AdapterRequest) -> Result<&str, ProxyError> {
    request
        .actual_model
        .as_deref()
        .or(request.model.as_deref())
        .or(request.requested_model.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProxyError::bad_request("Gemini v1internal requires a model"))
}

fn gemini_request_has_google_search(value: &Value) -> bool {
    value
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            tools.iter().any(|tool| {
                tool.get("googleSearch")
                    .or_else(|| tool.get("google_search"))
                    .is_some_and(Value::is_object)
            })
        })
}

fn gemini_ai_studio_base_url(_account: &Account) -> &str {
    #[cfg(test)]
    if let Some(base_url) = _account
        .raw
        .as_ref()
        .and_then(|raw| raw.get("testGeminiAiStudioBaseUrl"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return base_url;
    }
    GEMINI_AI_STUDIO_BASE_URL
}

fn gemini_ai_studio_count_tokens_endpoint(base_url: &str, model: &str) -> String {
    join_upstream_url(
        base_url,
        &format!(
            "/v1beta/models/{}:countTokens",
            percent_encode_path_segment(model)
        ),
    )
}

fn sanitize_gemini_v1internal_request(
    value: &mut Value,
    inject_antigravity_identity: bool,
) -> Result<(), ProxyError> {
    if !value.is_object() {
        return Err(ProxyError::bad_request(
            "Gemini v1internal request must be a JSON object",
        ));
    }
    normalize_gemini_tool_schemas(value);
    let object = value.as_object_mut().ok_or_else(|| {
        ProxyError::bad_request("Gemini v1internal request must be a JSON object")
    })?;
    for field in ["model", "project", "projectId", "project_id", "stream"] {
        object.remove(field);
    }
    if !inject_antigravity_identity {
        return Ok(());
    }
    object.remove("safetySettings");
    enable_antigravity_mixed_tool_invocations(object);
    let identity_present = object
        .get("systemInstruction")
        .and_then(|value| value.get("parts"))
        .and_then(Value::as_array)
        .is_some_and(|parts| {
            parts.iter().any(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.contains("You are Antigravity"))
            })
        });
    if identity_present {
        return Ok(());
    }
    let system_instruction = object
        .entry("systemInstruction".to_string())
        .or_insert_with(|| json!({"parts": []}));
    if !system_instruction.is_object() {
        *system_instruction = json!({"parts": []});
    }
    let system_instruction = system_instruction
        .as_object_mut()
        .expect("systemInstruction was normalized to an object");
    let parts = system_instruction
        .entry("parts".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if !parts.is_array() {
        *parts = Value::Array(Vec::new());
    }
    parts
        .as_array_mut()
        .expect("systemInstruction.parts was normalized to an array")
        .insert(0, json!({"text": ANTIGRAVITY_IDENTITY_PATCH}));
    Ok(())
}

fn enable_antigravity_mixed_tool_invocations(request: &mut serde_json::Map<String, Value>) {
    let mut has_google_search = false;
    let mut has_function_declarations = false;
    if let Some(tools) = request.get("tools").and_then(Value::as_array) {
        for tool in tools {
            has_google_search |= tool
                .get("googleSearch")
                .or_else(|| tool.get("google_search"))
                .is_some_and(Value::is_object);
            has_function_declarations |= tool
                .get("functionDeclarations")
                .or_else(|| tool.get("function_declarations"))
                .and_then(Value::as_array)
                .is_some_and(|declarations| !declarations.is_empty());
        }
    }
    if !has_google_search || !has_function_declarations {
        return;
    }

    let tool_config = request
        .entry("toolConfig".to_string())
        .or_insert_with(|| json!({}));
    if !tool_config.is_object() {
        *tool_config = json!({});
    }
    let tool_config = tool_config
        .as_object_mut()
        .expect("toolConfig was normalized to an object");
    tool_config.remove("include_server_side_tool_invocations");
    tool_config.insert(
        "includeServerSideToolInvocations".to_string(),
        Value::Bool(true),
    );
}

fn select_antigravity_inference_endpoint(endpoint: &mut String, account: &Account) {
    let Ok(parsed) = url::Url::parse(endpoint) else {
        return;
    };
    if !matches!(
        parsed.host_str(),
        Some("cloudcode-pa.googleapis.com" | "daily-cloudcode-pa.googleapis.com")
    ) {
        return;
    }
    let base_url = if antigravity_account_has_paid_tier(account) {
        ANTIGRAVITY_DAILY_BASE_URL
    } else {
        ANTIGRAVITY_PRODUCTION_BASE_URL
    };
    *endpoint = gemini_v1internal_endpoint(base_url);
}

fn antigravity_account_has_paid_tier(account: &Account) -> bool {
    if account
        .profile
        .as_ref()
        .and_then(|profile| profile.get("selectedTierSource"))
        .and_then(Value::as_str)
        .is_some_and(|source| source.eq_ignore_ascii_case("paidTier"))
    {
        return true;
    }

    std::iter::once(account.subscription_level.as_deref())
        .chain(account.profile.as_ref().into_iter().map(|profile| {
            ["/tier", "/subscriptionTier", "/selectedTier", "/planType"]
                .into_iter()
                .find_map(|pointer| profile.pointer(pointer).and_then(Value::as_str))
        }))
        .flatten()
        .any(antigravity_tier_is_paid)
}

fn antigravity_tier_is_paid(value: &str) -> bool {
    matches!(
        value
            .trim()
            .to_ascii_lowercase()
            .replace(['-', '_'], " ")
            .as_str(),
        "pro" | "ultra" | "google ai pro" | "google ai ultra"
    )
}

fn gemini_v1internal_endpoint(base_url: &str) -> String {
    join_upstream_url(base_url, &gemini_v1internal_path())
}

fn gemini_v1internal_path() -> String {
    "/v1internal:streamGenerateContent?alt=sse".to_string()
}

fn random_uuid_like() -> String {
    let mut bytes = [0_u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn apply_bedrock_bearer_forward_contract(
    plan: &ProviderRuntimePlan,
    request: &mut AdapterRequest,
) -> Result<(), ProxyError> {
    let parsed = url::Url::parse(&plan.endpoint)
        .map_err(|error| ProxyError::bad_request(format!("invalid Bedrock endpoint: {error}")))?;
    if parsed.host_str().is_none() {
        return Err(ProxyError::bad_request("Bedrock endpoint has no host"));
    }
    let model = request
        .actual_model
        .as_deref()
        .or(request.model.as_deref())
        .ok_or_else(|| ProxyError::bad_request("Bedrock model is required"))?;
    let operation = if request.upstream_stream_requested {
        "converse-stream"
    } else {
        "converse"
    };
    let path = format!(
        "/model/{}/{}",
        percent_encode_path_segment(model),
        operation
    );
    let mut endpoint = parsed;
    endpoint.set_path(&path);
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    let value = serde_json::from_slice::<Value>(&request.body).map_err(|error| {
        ProxyError::bad_request(format!("bedrock converse body must be valid json: {error}"))
    })?;
    let value = bedrock_converse_body_from_anthropic(&value)?;
    request.body = serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|error| ProxyError::bad_request(format!("encode Bedrock body failed: {error}")))?;
    request.upstream_endpoint = Some(endpoint.to_string());
    request.upstream_headers.retain(|(name, _)| {
        !matches!(
            name.to_ascii_lowercase().as_str(),
            "authorization" | "x-amz-date" | "x-amz-security-token"
        )
    });
    Ok(())
}

fn sigv4_dates_now() -> (String, String) {
    let now = chrono::Utc::now();
    (
        now.format("%Y%m%d").to_string(),
        now.format("%Y%m%dT%H%M%SZ").to_string(),
    )
}

fn bedrock_sigv4_request_plan(
    stored: &StoredProvider,
    request: &AdapterRequest,
    date_yyyymmdd: &str,
    amz_date: &str,
) -> Result<BedrockSigV4RequestPlan, ProxyError> {
    let provider = &stored.provider;
    let base_url = app_configured_base_url(provider, AppKind::Claude)
        .or_else(|| default_base_url(ProviderType::AwsBedrock))
        .ok_or_else(|| ProxyError::bad_request("bedrock base url is not configured"))?;
    let region = setting(provider, &["AWS_REGION"])
        .or_else(|| bedrock_region_from_base_url(&base_url))
        .ok_or_else(|| ProxyError::bad_request("AWS_REGION is required for Bedrock SigV4"))?;
    let access_key = setting(provider, &["AWS_ACCESS_KEY_ID"]).ok_or_else(|| {
        ProxyError::bad_request("AWS_ACCESS_KEY_ID is required for Bedrock SigV4")
    })?;
    let _secret_key = setting(provider, &["AWS_SECRET_ACCESS_KEY"]).ok_or_else(|| {
        ProxyError::bad_request("AWS_SECRET_ACCESS_KEY is required for Bedrock SigV4")
    })?;
    let session_token = setting(provider, &["AWS_SESSION_TOKEN"]);
    let host = bedrock_host(&base_url, &region)?;
    let model = request
        .actual_model
        .as_deref()
        .or(request.model.as_deref())
        .unwrap_or("anthropic.claude-sonnet-4-6");
    let operation = if request.upstream_stream_requested {
        "converse-stream"
    } else {
        "converse"
    };
    let canonical_uri = format!(
        "/model/{}/{}",
        percent_encode_path_segment(model),
        operation
    );
    let endpoint = format!("https://{host}{canonical_uri}");
    let body = serde_json::from_slice::<Value>(&request.body)
        .map_err(|error| {
            ProxyError::bad_request(format!("bedrock converse body must be valid json: {error}"))
        })
        .and_then(|value| bedrock_converse_body_from_anthropic(&value))?;
    let body_bytes = serde_json::to_vec(&body).map_err(|error| {
        ProxyError::bad_request(format!("bedrock converse body encode failed: {error}"))
    })?;
    let payload_hash = sha256_hex(&body_bytes);
    let mut canonical_headers = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("host".to_string(), host.clone()),
        ("x-amz-content-sha256".to_string(), payload_hash.clone()),
        ("x-amz-date".to_string(), amz_date.to_string()),
    ];
    if let Some(session_token) = session_token.as_ref() {
        canonical_headers.push(("x-amz-security-token".to_string(), session_token.clone()));
    }
    let signed_headers = canonical_headers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let canonical_headers_text = canonical_headers
        .iter()
        .map(|(name, value)| format!("{name}:{}\n", value.trim()))
        .collect::<String>();
    let canonical_request = format!(
        "POST\n{canonical_uri}\n\n{canonical_headers_text}\n{signed_headers}\n{payload_hash}"
    );
    let canonical_request_hash = sha256_hex(canonical_request.as_bytes());
    let service = "bedrock";
    let credential_scope = format!("{date_yyyymmdd}/{region}/{service}/aws4_request");
    let string_to_sign =
        format!("AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{canonical_request_hash}");
    let signature = if amz_date.is_empty() {
        "<not-signed>".to_string()
    } else {
        hex_lower(&hmac_sha256(
            &aws_sigv4_signing_key(&_secret_key, date_yyyymmdd, &region, service)?,
            string_to_sign.as_bytes(),
        )?)
    };
    let authorization_header = format!(
        "AWS4-HMAC-SHA256 Credential={access_key}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}",
    );
    let redacted_authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature=<redacted>",
        redact_aws_access_key(&access_key),
        credential_scope,
        signed_headers
    );

    Ok(BedrockSigV4RequestPlan {
        endpoint,
        region,
        service,
        host,
        canonical_uri,
        amz_date: amz_date.to_string(),
        body,
        credential_scope,
        signed_headers,
        payload_hash,
        canonical_request_hash,
        authorization_header,
        redacted_authorization,
        redacted_session_token: session_token.map(|_| "<redacted>".to_string()),
        signing_status: if amz_date.is_empty() {
            "canonical_request_only"
        } else {
            "sigv4_signed"
        },
    })
}

fn bedrock_sigv4_signed_request_parts(
    stored: &StoredProvider,
    request: &AdapterRequest,
    date_yyyymmdd: &str,
    amz_date: &str,
) -> Result<BedrockSignedRequestParts, ProxyError> {
    let plan = bedrock_sigv4_request_plan(stored, request, date_yyyymmdd, amz_date)?;
    let body = serde_json::to_vec(&plan.body).map_err(|error| {
        ProxyError::bad_request(format!("bedrock signed body encode failed: {error}"))
    })?;
    let mut headers = vec![
        ("authorization", plan.authorization_header.clone()),
        ("content-type", "application/json".to_string()),
        ("host", plan.host.clone()),
        ("x-amz-content-sha256", plan.payload_hash.clone()),
        ("x-amz-date", plan.amz_date.clone()),
    ];
    if let Some(session_token) = setting(&stored.provider, &["AWS_SESSION_TOKEN"]) {
        headers.push(("x-amz-security-token", session_token));
    }

    Ok(BedrockSignedRequestParts {
        endpoint: plan.endpoint.clone(),
        body,
        headers,
        plan,
    })
}

fn bedrock_converse_body_from_anthropic(input: &Value) -> Result<Value, ProxyError> {
    let messages = input
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| ProxyError::bad_request("bedrock converse requires messages array"))?;
    let mut output = serde_json::Map::new();
    output.insert(
        "messages".to_string(),
        Value::Array(
            messages
                .iter()
                .map(bedrock_message_from_anthropic)
                .collect::<Vec<_>>(),
        ),
    );

    let system = bedrock_system_from_anthropic(input.get("system"));
    if !system.is_empty() {
        output.insert("system".to_string(), Value::Array(system));
    }

    let inference_config = bedrock_inference_config_from_anthropic(input);
    if !inference_config.is_empty() {
        output.insert(
            "inferenceConfig".to_string(),
            Value::Object(inference_config),
        );
    }

    if let Some(tool_config) = bedrock_tool_config_from_anthropic(input)? {
        output.insert("toolConfig".to_string(), tool_config);
    }

    Ok(Value::Object(output))
}

fn bedrock_tool_config_from_anthropic(input: &Value) -> Result<Option<Value>, ProxyError> {
    let Some(tools) = input.get("tools") else {
        return Ok(None);
    };
    let tools = tools
        .as_array()
        .ok_or_else(|| ProxyError::bad_request("bedrock converse tools must be an array"))?;
    if tools.is_empty() {
        return Ok(None);
    }

    let tools = tools
        .iter()
        .map(|tool| {
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| ProxyError::bad_request("bedrock converse tool requires a name"))?;
            let input_schema = tool
                .get("input_schema")
                .filter(|schema| schema.is_object())
                .cloned()
                .ok_or_else(|| {
                    ProxyError::bad_request(
                        "bedrock converse tool input_schema must be a JSON object",
                    )
                })?;
            let mut tool_spec = serde_json::Map::new();
            tool_spec.insert("name".to_string(), Value::String(name.to_string()));
            if let Some(description) = tool
                .get("description")
                .and_then(Value::as_str)
                .filter(|description| !description.is_empty())
            {
                tool_spec.insert(
                    "description".to_string(),
                    Value::String(description.to_string()),
                );
            }
            tool_spec.insert("inputSchema".to_string(), json!({ "json": input_schema }));
            Ok(json!({ "toolSpec": tool_spec }))
        })
        .collect::<Result<Vec<_>, ProxyError>>()?;

    let mut tool_config = serde_json::Map::new();
    tool_config.insert("tools".to_string(), Value::Array(tools));
    if let Some(tool_choice) = input.get("tool_choice") {
        tool_config.insert(
            "toolChoice".to_string(),
            bedrock_tool_choice_from_anthropic(tool_choice)?,
        );
    }
    Ok(Some(Value::Object(tool_config)))
}

fn bedrock_tool_choice_from_anthropic(tool_choice: &Value) -> Result<Value, ProxyError> {
    match tool_choice.get("type").and_then(Value::as_str) {
        Some("auto") => Ok(json!({ "auto": {} })),
        Some("any") => Ok(json!({ "any": {} })),
        Some("tool") => {
            let name = tool_choice
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| {
                    ProxyError::bad_request("bedrock converse tool choice requires a name")
                })?;
            Ok(json!({ "tool": { "name": name } }))
        }
        Some(choice_type) => Err(ProxyError::bad_request(format!(
            "bedrock converse does not support tool choice {choice_type}"
        ))),
        None => Err(ProxyError::bad_request(
            "bedrock converse tool_choice requires a type",
        )),
    }
}

fn bedrock_message_from_anthropic(message: &Value) -> Value {
    let role = match message.get("role").and_then(Value::as_str) {
        Some("assistant") => "assistant",
        _ => "user",
    };
    json!({
        "role": role,
        "content": bedrock_content_from_anthropic(message.get("content")),
    })
}

fn bedrock_system_from_anthropic(system: Option<&Value>) -> Vec<Value> {
    match system {
        Some(Value::String(text)) if !text.is_empty() => vec![json!({ "text": text })],
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| {
                block
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                    .map(|text| json!({ "text": text }))
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn bedrock_content_from_anthropic(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::String(text)) => vec![json!({ "text": text })],
        Some(Value::Array(blocks)) => {
            let converted = blocks
                .iter()
                .filter_map(bedrock_content_block_from_anthropic)
                .collect::<Vec<_>>();
            if converted.is_empty() {
                vec![json!({ "text": "" })]
            } else {
                converted
            }
        }
        _ => vec![json!({ "text": "" })],
    }
}

fn bedrock_content_block_from_anthropic(block: &Value) -> Option<Value> {
    match block.get("type").and_then(Value::as_str) {
        Some("text") => block
            .get("text")
            .and_then(Value::as_str)
            .map(|text| json!({ "text": text })),
        Some("image") => {
            let source = block.get("source")?;
            let data = source.get("data").and_then(Value::as_str)?;
            let format = source
                .get("media_type")
                .and_then(Value::as_str)
                .and_then(|media_type| media_type.rsplit('/').next())
                .unwrap_or("png");
            Some(json!({
                "image": {
                    "format": format,
                    "source": { "bytes": data }
                }
            }))
        }
        Some("tool_use") => {
            let id = block.get("id").and_then(Value::as_str)?;
            let name = block.get("name").and_then(Value::as_str)?;
            Some(json!({
                "toolUse": {
                    "toolUseId": id,
                    "name": name,
                    "input": block.get("input").cloned().unwrap_or_else(|| json!({}))
                }
            }))
        }
        Some("tool_result") => {
            let tool_use_id = block.get("tool_use_id").and_then(Value::as_str)?;
            let status = if block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "error"
            } else {
                "success"
            };
            Some(json!({
                "toolResult": {
                    "toolUseId": tool_use_id,
                    "content": bedrock_tool_result_content(block.get("content")),
                    "status": status
                }
            }))
        }
        _ => None,
    }
}

fn bedrock_tool_result_content(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::String(text)) => vec![json!({ "text": text })],
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| {
                block
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|text| json!({ "text": text }))
            })
            .collect(),
        Some(value) => vec![json!({ "json": value })],
        None => vec![json!({ "text": "" })],
    }
}

fn bedrock_inference_config_from_anthropic(input: &Value) -> serde_json::Map<String, Value> {
    let mut config = serde_json::Map::new();
    if let Some(value) = input.get("max_tokens").and_then(Value::as_u64) {
        config.insert("maxTokens".to_string(), json!(value));
    }
    if let Some(value) = input.get("temperature").and_then(Value::as_f64) {
        config.insert("temperature".to_string(), json!(value));
    }
    if let Some(value) = input.get("top_p").and_then(Value::as_f64) {
        config.insert("topP".to_string(), json!(value));
    }
    if let Some(value) = input.get("stop_sequences").and_then(Value::as_array) {
        config.insert("stopSequences".to_string(), Value::Array(value.clone()));
    }
    config
}

fn bedrock_region_from_base_url(base_url: &str) -> Option<String> {
    let host = host_from_url(base_url)?;
    let marker = "bedrock-runtime.";
    let start = host.find(marker)? + marker.len();
    let rest = &host[start..];
    let region = rest.split('.').next()?.trim();
    (!region.is_empty() && !region.contains("${")).then(|| region.to_string())
}

fn bedrock_host(base_url: &str, region: &str) -> Result<String, ProxyError> {
    let normalized = base_url.replace("${AWS_REGION}", region);
    host_from_url(&normalized)
        .ok_or_else(|| ProxyError::bad_request("Bedrock base url must include a host"))
}

fn host_from_url(url: &str) -> Option<String> {
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let host = without_scheme.split('/').next()?.trim();
    (!host.is_empty()).then(|| host.to_string())
}

fn percent_encode_path_segment(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut output = String::new();
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'~') {
            output.push(*byte as char);
        } else {
            output.push('%');
            output.push(HEX[(byte >> 4) as usize] as char);
            output.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    output
}

fn redact_aws_access_key(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() <= 8 {
        return "<redacted>".to_string();
    }
    format!("{}...{}", &trimmed[..4], &trimmed[trimmed.len() - 4..])
}

type HmacSha256 = Hmac<Sha256>;

fn sha256_hex(value: &[u8]) -> String {
    let digest = Sha256::digest(value);
    hex_lower(&digest)
}

fn aws_sigv4_signing_key(
    secret_key: &str,
    date_yyyymmdd: &str,
    region: &str,
    service: &str,
) -> Result<Vec<u8>, ProxyError> {
    let date_key = hmac_sha256(
        format!("AWS4{secret_key}").as_bytes(),
        date_yyyymmdd.as_bytes(),
    )?;
    let region_key = hmac_sha256(&date_key, region.as_bytes())?;
    let service_key = hmac_sha256(&region_key, service.as_bytes())?;
    hmac_sha256(&service_key, b"aws4_request")
}

fn hmac_sha256(key: &[u8], value: &[u8]) -> Result<Vec<u8>, ProxyError> {
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|error| ProxyError::bad_request(format!("SigV4 HMAC setup failed: {error}")))?;
    mac.update(value);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn hex_lower(value: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(value.len() * 2);
    for byte in value {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn apply_auth_headers(
    headers: &mut Vec<(&'static str, String)>,
    app: AppKind,
    stored: &StoredProvider,
    accounts: &AccountStore,
) -> Result<(), ProxyError> {
    let provider = &stored.provider;
    let configured_provider_secret = match app {
        AppKind::Claude => setting(
            provider,
            &["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY", "API_KEY"],
        ),
        AppKind::Codex => super::codex_provider_api_key(provider),
        AppKind::Gemini => setting(provider, &["GEMINI_API_KEY", "GOOGLE_API_KEY", "API_KEY"]),
    };
    let managed_account_provider_type = managed_account_provider_type(stored);
    let provider_secret = managed_account_provider_type
        .is_none()
        .then_some(configured_provider_secret)
        .flatten();
    let provider_secret_configured = provider_secret.is_some();
    let (bound_account, account_credential) =
        if let Some(account_provider_type) = managed_account_provider_type {
            let (_, account_id) = managed_account_binding(stored).ok_or_else(|| {
                ProxyError::bad_request(format!(
                    "{} managed account binding is required",
                    account_provider_type.as_str()
                ))
            })?;
            let expected_identity_generation = provider
                .meta
                .as_ref()
                .and_then(|meta| meta.auth_binding.as_ref())
                .and_then(|binding| binding.auth_identity_generation)
                .ok_or_else(|| {
                    ProxyError::bad_request(format!(
                        "{} managed account binding identity generation is required",
                        account_provider_type.as_str()
                    ))
                })?;
            let account = accounts
                .find_for_provider(account_provider_type, Some(account_id))
                .ok_or_else(|| {
                    ProxyError::bad_request(format!(
                        "bound {} account {account_id} was not found",
                        account_provider_type.as_str()
                    ))
                })?;
            if account.auth_identity_generation != expected_identity_generation {
                return Err(ProxyError::conflict(format!(
                    "bound account {} identity changed; rebind the Provider",
                    account.id
                )));
            }
            let credential = manager_for(account_provider_type)
                .get_valid_token(
                    accounts,
                    account_provider_type,
                    Some(account_id),
                    now_ms_i64(),
                )
                .map_err(|error| ProxyError {
                    status: axum::http::StatusCode::UNAUTHORIZED,
                    message: format!(
                        "{} managed account credential is unavailable: {error}",
                        account_provider_type.as_str()
                    ),
                })?;
            (Some(account), Some(credential))
        } else {
            (None, None)
        };

    match app {
        AppKind::Claude => {
            if let Some(token) = provider_secret.or_else(|| {
                account_credential
                    .as_ref()
                    .map(|credential| credential.value.clone())
            }) {
                if stored.provider_type == ProviderType::Claude {
                    headers.push(("x-api-key", token));
                } else {
                    headers.push(("authorization", format!("Bearer {token}")));
                }
            }
        }
        AppKind::Codex => {
            if let Some(token) = provider_secret.or_else(|| {
                account_credential
                    .as_ref()
                    .map(|credential| credential.value.clone())
            }) {
                headers.push(("authorization", format!("Bearer {token}")));
            }
            if matches!(
                stored.provider_type,
                ProviderType::CodexOAuth | ProviderType::GrokOAuth
            ) && !provider_secret_configured
                && account_credential.is_none()
            {
                return Err(ProxyError::bad_request(format!(
                    "{} managed account access token is required",
                    stored.provider_type.as_str()
                )));
            }
            if stored.provider_type == ProviderType::CodexOAuth && !provider_secret_configured {
                if account_credential.is_none() {
                    return Err(ProxyError::bad_request(
                        "codex_oauth managed account access token is required",
                    ));
                }
                let account = bound_account.ok_or_else(|| {
                    ProxyError::bad_request("codex_oauth managed account binding is required")
                })?;
                let account_id = codex_oauth_chatgpt_account_id(account).ok_or_else(|| {
                    ProxyError::bad_request(
                        "codex_oauth managed account profile is missing chatgpt_account_id",
                    )
                })?;
                headers.push(("chatgpt-account-id", account_id));
                headers.push((
                    "originator",
                    crate::codex_identity::DEFAULT_CODEX_ORIGINATOR.to_string(),
                ));
                headers.push(("version", crate::codex_identity::configured_version()));
            }
        }
        AppKind::Gemini => {
            if let Some(key) = provider_secret {
                headers.push(("x-goog-api-key", key));
            } else if let Some(credential) = account_credential {
                match credential.credential_kind {
                    CredentialKind::AccessToken => {
                        headers.push(("authorization", format!("Bearer {}", credential.value)));
                    }
                    CredentialKind::ApiKey => headers.push(("x-goog-api-key", credential.value)),
                }
            }
        }
    }

    apply_common_provider_headers(headers, provider, stored.provider_type);
    Ok(())
}

fn codex_oauth_chatgpt_account_id(
    account: &crate::domain::accounts::store::Account,
) -> Option<String> {
    crate::domain::accounts::store::effective_codex_workspace_id(account)
}

fn apply_common_provider_headers(
    headers: &mut Vec<(&'static str, String)>,
    provider: &crate::domain::providers::model::Provider,
    provider_type: ProviderType,
) {
    if provider_type == ProviderType::OpenRouter {
        if let Some(referer) = setting(provider, &["OPENROUTER_SITE_URL", "HTTP_REFERER"]) {
            headers.push(("http-referer", referer));
        }
        if let Some(title) = setting(provider, &["OPENROUTER_APP_NAME", "X_TITLE"]) {
            headers.push(("x-title", title));
        }
    }
}

fn now_ms_i64() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub(super) fn codex_config_base_url(
    provider: &crate::domain::providers::model::Provider,
) -> Option<String> {
    let config = provider
        .settings_config
        .get("config")
        .and_then(Value::as_str)?;
    let marker = "base_url";
    let start = config.find(marker)?;
    let after_marker = &config[start + marker.len()..];
    let quote_start = after_marker.find('"')?;
    let after_quote = &after_marker[quote_start + 1..];
    let quote_end = after_quote.find('"')?;
    Some(after_quote[..quote_end].to_string())
}

#[derive(Debug, Clone, Default)]
pub(super) struct ModelSelection {
    pub requested_model: Option<String>,
    pub actual_model: Option<String>,
    pub actual_model_source: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct ModelMappingContext {
    app: AppKind,
    route: Option<ProxyRoute>,
    provider_type: ProviderType,
}

#[derive(Debug, Clone)]
struct ModelMappingDecision {
    actual_model: String,
    source: &'static str,
}

fn cache_injection_config(stored: &StoredProvider) -> CacheInjectionConfig {
    if stored.resource.profile_id.is_some() {
        return CacheInjectionConfig::disabled();
    }
    let settings = &stored.provider.settings_config;
    let cache_settings = settings
        .get("cacheInjection")
        .or_else(|| settings.get("cache_injection"));
    let enabled = cache_settings
        .and_then(|value| {
            value_as_bool(value).or_else(|| {
                bool_field(
                    value,
                    &[
                        "enabled",
                        "cacheInjection",
                        "cache_injection",
                        "promptCache",
                    ],
                )
            })
        })
        .or_else(|| {
            bool_field(
                settings,
                &[
                    "cacheInjectionEnabled",
                    "cache_injection_enabled",
                    "promptCacheEnabled",
                    "prompt_cache_enabled",
                ],
            )
        })
        .or_else(|| {
            meta_extra_value(stored, &["promptCacheEnabled", "prompt_cache_enabled"])
                .and_then(value_as_bool)
        })
        .unwrap_or(false);

    if !enabled {
        return CacheInjectionConfig::disabled();
    }

    let ttl = cache_settings
        .and_then(|value| string_field(value, &["ttl", "cacheTtl", "cache_ttl"]))
        .or_else(|| {
            string_field(
                settings,
                &[
                    "cacheInjectionTtl",
                    "cache_injection_ttl",
                    "promptCacheTtl",
                    "prompt_cache_ttl",
                ],
            )
        })
        .or_else(|| {
            meta_extra_value(stored, &["promptCacheTtl", "prompt_cache_ttl"])
                .and_then(value_as_string)
        })
        .unwrap_or_else(|| "5m".to_string());

    CacheInjectionConfig { enabled, ttl }
}

fn thinking_pipeline_config(stored: &StoredProvider) -> ThinkingPipelineConfig {
    if stored.resource.profile_id.is_some() {
        return ThinkingPipelineConfig::disabled();
    }
    let settings = &stored.provider.settings_config;
    let thinking_settings = settings
        .get("thinkingPipeline")
        .or_else(|| settings.get("thinking_pipeline"))
        .or_else(|| settings.get("thinking"));
    let master = thinking_settings
        .and_then(|value| {
            value_as_bool(value).or_else(|| {
                bool_field(value, &["enabled", "thinkingPipeline", "thinking_pipeline"])
            })
        })
        .or_else(|| {
            bool_field(
                settings,
                &["thinkingPipelineEnabled", "thinking_pipeline_enabled"],
            )
        })
        .or_else(|| {
            meta_extra_value(
                stored,
                &["thinkingPipelineEnabled", "thinking_pipeline_enabled"],
            )
            .and_then(value_as_bool)
        });

    if master == Some(false) {
        return ThinkingPipelineConfig::disabled();
    }

    let optimizer_enabled = thinking_settings
        .and_then(|value| {
            bool_field(
                value,
                &[
                    "optimizer",
                    "thinkingOptimizer",
                    "thinking_optimizer",
                    "optimize",
                    "optimizeThinking",
                    "optimize_thinking",
                ],
            )
        })
        .or_else(|| {
            bool_field(
                settings,
                &[
                    "thinkingOptimizer",
                    "thinking_optimizer",
                    "optimizeThinking",
                ],
            )
        })
        .or_else(|| {
            meta_extra_value(stored, &["thinkingOptimizer", "thinking_optimizer"])
                .and_then(value_as_bool)
        })
        .unwrap_or(master.unwrap_or(false));
    let signature_rectifier_enabled = thinking_settings
        .and_then(|value| {
            bool_field(
                value,
                &[
                    "signatureRectifier",
                    "signature_rectifier",
                    "requestThinkingSignature",
                    "request_thinking_signature",
                    "rectifySignature",
                    "rectify_signature",
                ],
            )
        })
        .or_else(|| {
            bool_field(
                settings,
                &[
                    "thinkingSignatureRectifier",
                    "thinking_signature_rectifier",
                    "requestThinkingSignature",
                    "request_thinking_signature",
                ],
            )
        })
        .or_else(|| {
            meta_extra_value(
                stored,
                &[
                    "thinkingSignatureRectifier",
                    "thinking_signature_rectifier",
                    "requestThinkingSignature",
                    "request_thinking_signature",
                ],
            )
            .and_then(value_as_bool)
        })
        .unwrap_or(false);
    let budget_rectifier_enabled = thinking_settings
        .and_then(|value| {
            bool_field(
                value,
                &[
                    "budgetRectifier",
                    "budget_rectifier",
                    "requestThinkingBudget",
                    "request_thinking_budget",
                    "rectifyBudget",
                    "rectify_budget",
                ],
            )
        })
        .or_else(|| {
            bool_field(
                settings,
                &[
                    "thinkingBudgetRectifier",
                    "thinking_budget_rectifier",
                    "requestThinkingBudget",
                    "request_thinking_budget",
                ],
            )
        })
        .or_else(|| {
            meta_extra_value(
                stored,
                &[
                    "thinkingBudgetRectifier",
                    "thinking_budget_rectifier",
                    "requestThinkingBudget",
                    "request_thinking_budget",
                ],
            )
            .and_then(value_as_bool)
        })
        .unwrap_or(false);

    ThinkingPipelineConfig {
        optimizer_enabled,
        signature_rectifier_enabled,
        budget_rectifier_enabled,
    }
}

fn request_governance_config(stored: &StoredProvider) -> RequestGovernanceConfig {
    if stored.resource.profile_id.is_some() {
        return RequestGovernanceConfig::disabled();
    }
    let settings = &stored.provider.settings_config;
    let governance_settings = settings
        .get("requestGovernance")
        .or_else(|| settings.get("request_governance"));
    let master = governance_settings
        .and_then(|value| {
            value_as_bool(value).or_else(|| {
                bool_field(
                    value,
                    &["enabled", "requestGovernance", "request_governance"],
                )
            })
        })
        .or_else(|| {
            bool_field(
                settings,
                &["requestGovernanceEnabled", "request_governance_enabled"],
            )
        })
        .or_else(|| {
            meta_extra_value(
                stored,
                &["requestGovernanceEnabled", "request_governance_enabled"],
            )
            .and_then(value_as_bool)
        });

    if master == Some(false) {
        return RequestGovernanceConfig::disabled();
    }

    let body_filter_enabled = governance_settings
        .and_then(|value| {
            bool_field(
                value,
                &[
                    "bodyFilter",
                    "body_filter",
                    "filterPrivateParams",
                    "filter_private_params",
                ],
            )
        })
        .or_else(|| {
            bool_field(
                settings,
                &[
                    "requestBodyFilter",
                    "request_body_filter",
                    "filterPrivateParams",
                    "filter_private_params",
                ],
            )
        })
        .or_else(|| {
            meta_extra_value(
                stored,
                &[
                    "requestBodyFilter",
                    "request_body_filter",
                    "filterPrivateParams",
                    "filter_private_params",
                ],
            )
            .and_then(value_as_bool)
        })
        .unwrap_or(master.unwrap_or(false));
    let media_sanitizer_enabled = governance_settings
        .and_then(|value| {
            bool_field(
                value,
                &[
                    "mediaSanitizer",
                    "media_sanitizer",
                    "replaceImages",
                    "replace_images",
                ],
            )
        })
        .or_else(|| {
            bool_field(
                settings,
                &[
                    "requestMediaSanitizer",
                    "request_media_sanitizer",
                    "replaceImages",
                    "replace_images",
                ],
            )
        })
        .or_else(|| {
            meta_extra_value(
                stored,
                &[
                    "requestMediaSanitizer",
                    "request_media_sanitizer",
                    "replaceImages",
                    "replace_images",
                ],
            )
            .and_then(value_as_bool)
        })
        .unwrap_or(master.unwrap_or(false));
    let media_heuristic_enabled = governance_settings
        .and_then(|value| {
            bool_field(
                value,
                &[
                    "mediaHeuristic",
                    "media_heuristic",
                    "allowHeuristic",
                    "allow_heuristic",
                ],
            )
        })
        .or_else(|| {
            bool_field(
                settings,
                &[
                    "requestMediaHeuristic",
                    "request_media_heuristic",
                    "allowMediaHeuristic",
                    "allow_media_heuristic",
                ],
            )
        })
        .unwrap_or(false);
    let private_field_whitelist = governance_settings
        .and_then(|value| {
            string_array_field(
                value,
                &[
                    "privateFieldWhitelist",
                    "private_field_whitelist",
                    "whitelist",
                ],
            )
        })
        .or_else(|| {
            string_array_field(
                settings,
                &[
                    "requestPrivateFieldWhitelist",
                    "request_private_field_whitelist",
                    "privateFieldWhitelist",
                    "private_field_whitelist",
                ],
            )
        })
        .unwrap_or_default();

    RequestGovernanceConfig {
        body_filter_enabled,
        media_sanitizer_enabled,
        media_heuristic_enabled,
        private_field_whitelist,
    }
}

fn meta_extra_value<'a>(stored: &'a StoredProvider, keys: &[&str]) -> Option<&'a Value> {
    let meta = stored.provider.meta.as_ref()?;
    keys.iter().find_map(|key| meta.extra.get(*key))
}

fn apply_request_preprocessors(
    body: Bytes,
    stored: &StoredProvider,
    route: Option<ProxyRoute>,
) -> (Bytes, ModelSelection) {
    apply_model_mapping(
        body,
        &stored.provider.settings_config,
        ModelMappingContext {
            app: stored.app,
            route,
            provider_type: stored.provider_type,
        },
    )
}

pub(super) fn apply_provider_model_routing(
    body: Bytes,
    stored: &StoredProvider,
    route: ProxyRoute,
) -> (Bytes, ModelSelection) {
    apply_request_preprocessors(body, stored, Some(route))
}

fn maybe_apply_copilot_preflight(
    body: Bytes,
    stored: &StoredProvider,
    metadata: Option<&CopilotRequestMetadata>,
) -> Result<CopilotPreflightResult, ProxyError> {
    let mut value = serde_json::from_slice::<Value>(&body).map_err(|error| {
        ProxyError::bad_request(format!("copilot request body must be valid json: {error}"))
    })?;
    let mut model_source = None;
    if let Some(model) = value
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        if let Some(normalized) =
            normalize_or_resolve_model(model, &stored.provider.settings_config)
        {
            value["model"] = Value::String(normalized);
            model_source = Some("copilot_model_normalization");
        }
    }

    let config = CopilotOptimizerConfig::from_settings(&stored.provider.settings_config);
    let default_metadata;
    let metadata = match metadata {
        Some(metadata) => metadata,
        None => {
            default_metadata = CopilotRequestMetadata::default();
            &default_metadata
        }
    };
    let optimization = optimize_copilot_request(&mut value, &config, metadata);
    if optimization.model_source.is_some() {
        model_source = optimization.model_source;
    }
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map(|body| (body, optimization.headers, model_source))
        .map_err(|error| ProxyError::bad_request(format!("copilot request encode failed: {error}")))
}

fn model_from_body(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("model")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(str::to_string)
        })
}

fn apply_model_mapping(
    body: Bytes,
    settings: &Value,
    context: ModelMappingContext,
) -> (Bytes, ModelSelection) {
    let Ok(mut value) = serde_json::from_slice::<Value>(&body) else {
        return (body, ModelSelection::default());
    };
    let requested_model = value
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string);

    if let Some(decision) = resolve_model_mapping(settings, requested_model.as_deref(), context) {
        value["model"] = Value::String(decision.actual_model.clone());
        if let Ok(bytes) = serde_json::to_vec(&value) {
            return (
                Bytes::from(bytes),
                ModelSelection {
                    requested_model,
                    actual_model: Some(decision.actual_model.clone()),
                    actual_model_source: Some(decision.source.to_string()),
                },
            );
        }
    }

    (
        body,
        ModelSelection {
            requested_model: requested_model.clone(),
            actual_model: requested_model.clone(),
            actual_model_source: requested_model.as_ref().map(|_| "request".to_string()),
        },
    )
}

fn resolve_model_mapping(
    settings: &Value,
    requested_model: Option<&str>,
    context: ModelMappingContext,
) -> Option<ModelMappingDecision> {
    let mapping = settings
        .get("modelMapping")
        .or_else(|| settings.get("model_mapping"));
    if let Some(policy) = policy_from_settings(settings) {
        match policy.mode {
            ModelRoutingMode::Single => {
                if let Some(actual_model) = policy.upstream_model {
                    return Some(ModelMappingDecision {
                        actual_model,
                        source: "model_mapping_single",
                    });
                }
            }
            ModelRoutingMode::Passthrough => {
                return requested_model.map(|requested_model| ModelMappingDecision {
                    actual_model: requested_model.to_string(),
                    source: "model_mapping_passthrough",
                });
            }
        }
    }
    if let Some(requested_model) = requested_model {
        if let Some(decision) =
            mapping.and_then(|mapping| direct_model_mapping(mapping, requested_model))
        {
            return Some(decision);
        }
        if let Some(decision) = catalog_model_mapping(settings, requested_model, context) {
            return Some(decision);
        }
        if let Some(decision) =
            mapping.and_then(|mapping| rule_model_mapping(mapping, requested_model, context))
        {
            return Some(decision);
        }
        if let Some(decision) = env_model_mapping(settings, requested_model) {
            return Some(decision);
        }
    }
    mapping.and_then(legacy_upstream_model_mapping)
}

fn legacy_upstream_model_mapping(mapping: &Value) -> Option<ModelMappingDecision> {
    let actual_model = string_field(mapping, &["upstreamModel", "upstream_model"])?;
    Some(ModelMappingDecision {
        actual_model,
        source: "model_mapping",
    })
}

fn direct_model_mapping(mapping: &Value, requested_model: &str) -> Option<ModelMappingDecision> {
    direct_model_mapping_value(mapping.get(requested_model))
        .or_else(|| {
            mapping
                .get("mappings")
                .and_then(|mappings| direct_model_mapping_value(mappings.get(requested_model)))
        })
        .map(|actual_model| ModelMappingDecision {
            actual_model,
            source: "model_mapping_direct",
        })
}

fn direct_model_mapping_value(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(actual_model) = value
        .as_str()
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        return Some(actual_model.to_string());
    }
    let actual_model = string_field(
        value,
        &[
            "upstreamModel",
            "upstream_model",
            "actualModel",
            "actual_model",
            "targetModel",
            "target_model",
            "target",
            "to",
        ],
    )?;
    Some(actual_model)
}

fn catalog_model_mapping(
    settings: &Value,
    requested_model: &str,
    context: ModelMappingContext,
) -> Option<ModelMappingDecision> {
    let models = settings
        .get("modelCatalog")
        .or_else(|| settings.get("model_catalog"))
        .and_then(|catalog| catalog.get("models"))
        .and_then(Value::as_array)?;
    models.iter().find_map(|model| {
        if !context_matches(model, context) {
            return None;
        }
        let route_model = string_field(model, &["model", "id", "name"])?;
        if route_model != requested_model {
            return None;
        }
        let actual_model = string_field(model, &["upstreamModel", "upstream_model"])?;
        Some(ModelMappingDecision {
            actual_model,
            source: "model_catalog",
        })
    })
}

fn rule_model_mapping(
    mapping: &Value,
    requested_model: &str,
    context: ModelMappingContext,
) -> Option<ModelMappingDecision> {
    let rules = mapping
        .get("rules")
        .or_else(|| mapping.get("modelRules"))
        .or_else(|| mapping.get("model_rules"))
        .and_then(Value::as_array)?;
    rules.iter().find_map(|rule| {
        if rule.get("enabled").and_then(Value::as_bool) == Some(false) {
            return None;
        }
        if !context_matches(rule, context) || !rule_matches_model(rule, requested_model) {
            return None;
        }
        let actual_model = string_field(
            rule,
            &[
                "upstreamModel",
                "upstream_model",
                "actualModel",
                "actual_model",
                "targetModel",
                "target_model",
                "target",
                "to",
            ],
        )?;
        Some(ModelMappingDecision {
            actual_model,
            source: "model_mapping_rule",
        })
    })
}

fn env_model_mapping(settings: &Value, requested_model: &str) -> Option<ModelMappingDecision> {
    let env = settings.get("env")?;
    let requested = requested_model.to_ascii_lowercase();
    let actual_model = if requested.contains("fable") {
        string_field(env, &["ANTHROPIC_DEFAULT_FABLE_MODEL"])
            .or_else(|| string_field(env, &["ANTHROPIC_DEFAULT_OPUS_MODEL"]))
    } else if requested.contains("haiku") {
        string_field(env, &["ANTHROPIC_DEFAULT_HAIKU_MODEL"])
    } else if requested.contains("opus") {
        string_field(env, &["ANTHROPIC_DEFAULT_OPUS_MODEL"])
    } else if requested.contains("sonnet") {
        string_field(env, &["ANTHROPIC_DEFAULT_SONNET_MODEL"])
    } else {
        None
    }
    .or_else(|| {
        string_field(
            env,
            &[
                "ANTHROPIC_MODEL",
                "OPENAI_MODEL",
                "GEMINI_MODEL",
                "GOOGLE_GEMINI_MODEL",
            ],
        )
    })?;
    Some(ModelMappingDecision {
        actual_model,
        source: "model_mapping_env",
    })
}

fn context_matches(value: &Value, context: ModelMappingContext) -> bool {
    selector_matches(value, &["app", "apps"], context.app.as_str())
        && context
            .route
            .map(|route| route_selector_matches(value, &["route", "routes"], route))
            .unwrap_or_else(|| !has_any_key(value, &["route", "routes"]))
        && selector_matches(
            value,
            &[
                "providerType",
                "provider_type",
                "providerTypes",
                "provider_types",
            ],
            context.provider_type.as_str(),
        )
}

fn selector_matches(value: &Value, keys: &[&str], expected: &str) -> bool {
    let Some(selector) = keys.iter().find_map(|key| value.get(*key)) else {
        return true;
    };
    value_contains_string(selector, expected)
}

fn value_contains_string(value: &Value, expected: &str) -> bool {
    if let Some(item) = value.as_str() {
        return string_selector_matches(item, expected);
    }
    value.as_array().is_some_and(|items| {
        items
            .iter()
            .filter_map(Value::as_str)
            .any(|item| string_selector_matches(item, expected))
    })
}

fn route_selector_matches(value: &Value, keys: &[&str], route: ProxyRoute) -> bool {
    let Some(selector) = keys.iter().find_map(|key| value.get(*key)) else {
        return true;
    };
    if let Some(item) = selector.as_str() {
        return route_string_matches(item, route);
    }
    selector.as_array().is_some_and(|items| {
        items
            .iter()
            .filter_map(Value::as_str)
            .any(|item| route_string_matches(item, route))
    })
}

fn string_selector_matches(selector: &str, expected: &str) -> bool {
    let selector = selector.trim();
    selector == "*" || selector.eq_ignore_ascii_case(expected)
}

fn route_string_matches(selector: &str, route: ProxyRoute) -> bool {
    let selector = selector.trim();
    selector == "*"
        || route_aliases(route)
            .iter()
            .any(|alias| selector.eq_ignore_ascii_case(alias))
}

fn has_any_key(value: &Value, keys: &[&str]) -> bool {
    keys.iter().any(|key| value.get(*key).is_some())
}

fn rule_matches_model(rule: &Value, requested_model: &str) -> bool {
    let model_selectors = [
        "match",
        "pattern",
        "from",
        "sourceModel",
        "source_model",
        "requestedModel",
        "requested_model",
    ];
    if let Some(value) = model_selectors.iter().find_map(|key| rule.get(*key)) {
        return model_selector_matches(value, requested_model);
    }
    if let Some(value) = rule.get("models").or_else(|| rule.get("requestModels")) {
        return model_selector_matches(value, requested_model);
    }
    rule.get("default").and_then(Value::as_bool) == Some(true)
        || rule.get("fallback").and_then(Value::as_bool) == Some(true)
}

fn model_selector_matches(value: &Value, requested_model: &str) -> bool {
    if let Some(pattern) = value.as_str() {
        return wildcard_model_match(pattern, requested_model);
    }
    value.as_array().is_some_and(|items| {
        items
            .iter()
            .filter_map(Value::as_str)
            .any(|pattern| wildcard_model_match(pattern, requested_model))
    })
}

fn wildcard_model_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return false;
    }
    if pattern == "*" || pattern == value {
        return true;
    }
    if !pattern.contains('*') {
        return false;
    }

    let parts = pattern.split('*').collect::<Vec<_>>();
    let mut position = 0usize;
    if let Some(first) = parts.first().filter(|part| !part.is_empty()) {
        if !value.starts_with(first) {
            return false;
        }
        position = first.len();
    }

    for (index, part) in parts.iter().enumerate().skip(1) {
        if part.is_empty() {
            continue;
        }
        let is_last = index == parts.len() - 1;
        if is_last && !pattern.ends_with('*') {
            return value[position..].contains(part) && value.ends_with(part);
        }
        let Some(found) = value[position..].find(part) else {
            return false;
        };
        position += found + part.len();
    }
    true
}

fn route_aliases(route: ProxyRoute) -> &'static [&'static str] {
    match route {
        ProxyRoute::ClaudeMessages => &["claude_messages", "messages", "/v1/messages"],
        ProxyRoute::ClaudeCountTokens => &[
            "claude_count_tokens",
            "count_tokens",
            "/v1/messages/count_tokens",
        ],
        ProxyRoute::CodexChatCompletions => &[
            "codex_chat_completions",
            "chat_completions",
            "/v1/chat/completions",
        ],
        ProxyRoute::CodexResponses => &["codex_responses", "responses", "/v1/responses"],
        ProxyRoute::CodexResponsesCompact => &[
            "codex_responses_compact",
            "responses_compact",
            "/v1/responses/compact",
        ],
        ProxyRoute::Gemini => &["gemini", "generate_content", "stream_generate_content"],
    }
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
    })
}

fn bool_field(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(value_as_bool))
}

fn string_array_field(value: &Value, keys: &[&str]) -> Option<Vec<String>> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|items| {
            items.as_array().map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
        })
    })
}

fn value_as_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
}

fn value_as_bool(value: &Value) -> Option<bool> {
    if let Some(value) = value.as_bool() {
        return Some(value);
    }
    if let Some(value) = value.as_i64() {
        return match value {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
    }
    let value = value.as_str()?.trim().to_ascii_lowercase();
    if ["true", "1", "yes", "on", "enabled"].contains(&value.as_str()) {
        return Some(true);
    }
    if ["false", "0", "no", "off", "disabled"].contains(&value.as_str()) {
        return Some(false);
    }
    None
}

fn is_stream_requested(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value.get("stream").and_then(Value::as_bool))
        .unwrap_or(false)
}

fn route_implies_stream(route: ProxyRoute, gemini_path: Option<&str>) -> bool {
    route == ProxyRoute::Gemini
        && gemini_path.is_some_and(|path| {
            path.ends_with(":streamGenerateContent") || path.ends_with("streamGenerateContent")
        })
}

fn ensure_stream_enabled(
    body: Bytes,
    upstream_format: UpstreamFormat,
) -> Result<Bytes, ProxyError> {
    let mut value = serde_json::from_slice::<Value>(&body).map_err(|error| {
        ProxyError::bad_request(format!(
            "request body must be valid json to force stream: {error}"
        ))
    })?;
    let Value::Object(map) = &mut value else {
        return Ok(body);
    };
    map.insert("stream".to_string(), Value::Bool(true));
    if matches!(
        upstream_format,
        UpstreamFormat::OpenAiChat | UpstreamFormat::OpenAiResponses
    ) {
        let mut stream_options = serde_json::Map::new();
        stream_options.insert("include_usage".to_string(), Value::Bool(true));
        match map.get_mut("stream_options") {
            Some(Value::Object(options)) => {
                options.insert("include_usage".to_string(), Value::Bool(true));
            }
            _ => {
                map.insert("stream_options".to_string(), Value::Object(stream_options));
            }
        }
    }
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|error| ProxyError::bad_request(format!("request stream encode failed: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn response_error_detection_ignores_empty_placeholders() {
        for value in [
            json!({"error": null}),
            json!({"error": {}}),
            json!({"errors": []}),
            json!({"error_message": ""}),
            json!({"errorMessage": false}),
        ] {
            assert!(!looks_like_error_response(&value), "value={value}");
        }
        for value in [
            json!({"error": {"message": "failed"}}),
            json!({"errors": ["failed"]}),
            json!({"error_message": "failed"}),
        ] {
            assert!(looks_like_error_response(&value), "value={value}");
        }
    }

    #[test]
    fn qoder_profiles_canonicalize_all_three_app_surfaces_to_openai_chat() {
        let cases = [
            (
                AppKind::Claude,
                ProxyRoute::ClaudeMessages,
                None,
                br#"{"model":"auto","system":"claude-system","messages":[{"role":"user","content":"claude-user"}],"stream":false}"#.as_slice(),
                "claude-user",
            ),
            (
                AppKind::Codex,
                ProxyRoute::CodexResponses,
                None,
                br#"{"model":"auto","instructions":"codex-system","input":"codex-user","stream":false,"tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}}]}"#.as_slice(),
                "codex-user",
            ),
            (
                AppKind::Gemini,
                ProxyRoute::Gemini,
                Some("models/auto:generateContent"),
                br#"{"systemInstruction":{"parts":[{"text":"gemini-system"}]},"contents":[{"role":"user","parts":[{"text":"gemini-user"}]}]}"#.as_slice(),
                "gemini-user",
            ),
        ];

        for (app, route, gemini_path, body, expected_user) in cases {
            let mut stored = stored_provider(app, ProviderType::QoderCosy, json!({}));
            stored.resource.profile_id = Some(
                crate::domain::providers::registry::ProfileId::parse(match app {
                    AppKind::Claude => "claude.qoder_cosy",
                    AppKind::Codex => "codex.qoder_cosy",
                    AppKind::Gemini => "gemini.qoder_cosy",
                })
                .unwrap(),
            );
            let request = adapter_for(app, ProviderType::QoderCosy)
                .transform_request_for_route(
                    Bytes::copy_from_slice(body),
                    &stored,
                    route,
                    gemini_path,
                )
                .unwrap();
            let canonical: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(canonical["model"], "auto", "app={app:?}");
            assert!(canonical["messages"].is_array(), "app={app:?}");
            assert!(
                canonical["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|message| message["role"] == "user"
                        && value_contains_text(&message["content"], expected_user)),
                "app={app:?} canonical={canonical}"
            );
            assert!(canonical.get("contents").is_none(), "app={app:?}");
            assert!(canonical.get("input").is_none(), "app={app:?}");
            assert!(!request.stream_requested, "app={app:?}");
        }
    }

    fn value_contains_text(value: &Value, expected: &str) -> bool {
        match value {
            Value::String(value) => value == expected,
            Value::Array(values) => values
                .iter()
                .any(|value| value_contains_text(value, expected)),
            Value::Object(values) => values
                .values()
                .any(|value| value_contains_text(value, expected)),
            _ => false,
        }
    }

    struct AdapterContract<'a> {
        app: AppKind,
        provider_type: ProviderType,
        route: ProxyRoute,
        gemini_path: Option<String>,
        stored: StoredProvider,
        request_body: &'a [u8],
        expected_endpoint: &'a str,
        expected_header: (&'static str, &'a str),
        expected_model: Option<&'a str>,
        expected_stream: bool,
    }

    fn stored_provider(
        app: AppKind,
        provider_type: ProviderType,
        settings_config: Value,
    ) -> StoredProvider {
        StoredProvider {
            app,
            provider: crate::domain::providers::model::Provider {
                id: format!("{}-fixture", provider_type.as_str()),
                name: provider_type.as_str().to_string(),
                settings_config,
                category: None,
                meta: None,
                extra: Default::default(),
            },
            provider_type,
            provider_type_id: provider_type.as_str().to_string(),
            resource: Default::default(),
        }
    }

    #[test]
    fn minimax_coding_plan_normalizes_only_enabled_thinking_on_minimax_models() {
        let cases = [
            ("claude.minimax_cn", "MiniMax-M3", "enabled", "adaptive"),
            (
                "claude.minimax_global",
                "MiniMax-M2.7-highspeed",
                "enabled",
                "adaptive",
            ),
            ("claude.minimax_cn", "MiniMax-M2.7", "adaptive", "adaptive"),
            ("claude.minimax_cn", "glm-5", "enabled", "enabled"),
            ("claude.zhipu_glm_cn", "MiniMax-M3", "enabled", "enabled"),
        ];

        for (profile_id, model, input_type, expected_type) in cases {
            let mut stored = stored_provider(
                AppKind::Claude,
                ProviderType::Claude,
                json!({"apiKey": "fixture-secret"}),
            );
            stored.resource.profile_id =
                Some(crate::domain::providers::registry::ProfileId::parse(profile_id).unwrap());
            let plan = crate::domain::providers::runtime::compile_runtime_plan(
                &stored,
                &AccountStore::default(),
            )
            .unwrap();
            let body = serde_json::to_vec(&json!({
                "model": model,
                "max_tokens": 16_384,
                "thinking": {
                    "type": input_type,
                    "budget_tokens": 8_192,
                    "vendor_field": "preserve"
                },
                "messages": [{"role": "user", "content": "ping"}]
            }))
            .unwrap();
            let mut request = adapter_for(AppKind::Claude, ProviderType::Claude)
                .transform_request_for_route(
                    Bytes::from(body),
                    &stored,
                    ProxyRoute::ClaudeMessages,
                    None,
                )
                .unwrap();

            finalize_runtime_request(&plan, &stored, &mut request).unwrap();

            let normalized: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(
                normalized.pointer("/thinking/type"),
                Some(&json!(expected_type)),
                "profile={profile_id} model={model}"
            );
            assert_eq!(
                normalized.pointer("/thinking/budget_tokens"),
                Some(&json!(8_192))
            );
            assert_eq!(
                normalized.pointer("/thinking/vendor_field"),
                Some(&json!("preserve"))
            );
        }
    }

    fn codex_oauth_stored_provider_with_account_binding(account_id: &str) -> StoredProvider {
        let mut stored = stored_provider(AppKind::Codex, ProviderType::CodexOAuth, json!({}));
        stored.provider.meta = Some(crate::domain::providers::model::ProviderMeta {
            auth_binding: Some(crate::domain::providers::model::AuthBinding {
                source: Some("account".to_string()),
                auth_provider: Some("codex_oauth".to_string()),
                account_id: Some(account_id.to_string()),
                auth_identity_generation: Some(1),
            }),
            ..Default::default()
        });
        stored
    }

    fn codex_oauth_account(
        account_id: &str,
        access_token: &str,
        profile: Value,
    ) -> crate::domain::accounts::store::Account {
        crate::domain::accounts::store::Account {
            id: account_id.to_string(),
            provider_type: ProviderType::CodexOAuth,
            auth_identity_generation: 1,
            token_refresh_generation: 1,
            email: Some("codex@example.test".to_string()),
            access_token: Some(access_token.to_string()),
            refresh_token: Some("refresh-token".to_string()),
            id_token: None,
            token_type: Some("Bearer".to_string()),
            api_key: None,
            extra_headers: Default::default(),
            scopes: Vec::new(),
            profile: Some(profile),
            raw: None,
            subscription_level: None,
            entitlement_status: None,
            quota_percent: None,
            quota: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: None,
            manual_subscription_expires_at_ms: None,
            manual_subscription_expiry_updated_at_ms: None,
            manual_subscription_expiry_rule: None,
            rate_limited_until: None,
            last_refresh_error: None,
            refresh_consecutive_failures: 0,
            needs_relogin: false,
            capability_observations: Default::default(),
        }
    }

    fn managed_test_account(
        provider_type: ProviderType,
        account_id: &str,
        access_token: &str,
        profile: Value,
    ) -> crate::domain::accounts::store::Account {
        let mut account = codex_oauth_account(account_id, access_token, profile);
        account.provider_type = provider_type;
        account
    }

    fn bind_managed_test_provider(stored: &mut StoredProvider, account_id: &str) {
        stored.provider.meta = Some(crate::domain::providers::model::ProviderMeta {
            auth_binding: Some(crate::domain::providers::model::AuthBinding {
                source: Some("account".to_string()),
                auth_provider: Some(stored.provider_type.as_str().to_string()),
                account_id: Some(account_id.to_string()),
                auth_identity_generation: Some(1),
            }),
            ..Default::default()
        });
    }

    fn assert_adapter_contract(contract: AdapterContract<'_>) {
        assert_adapter_contract_with_accounts(contract, &AccountStore::default());
    }

    fn assert_adapter_contract_with_accounts(
        contract: AdapterContract<'_>,
        accounts: &AccountStore,
    ) {
        let adapter = adapter_for(contract.app, contract.provider_type);
        let headers = adapter
            .build_headers(contract.app, &contract.stored, accounts)
            .unwrap();
        assert!(headers.iter().any(|item| {
            item == &(
                contract.expected_header.0,
                contract.expected_header.1.to_string(),
            )
        }));

        let request = adapter
            .transform_request_for_route(
                Bytes::copy_from_slice(contract.request_body),
                &contract.stored,
                contract.route,
                contract.gemini_path.as_deref(),
            )
            .unwrap();
        let endpoint = adapter
            .resolve_endpoint_for_request(
                contract.route,
                contract.gemini_path,
                &contract.stored,
                &request,
            )
            .unwrap();
        assert_eq!(endpoint, contract.expected_endpoint);
        assert_eq!(request.model.as_deref(), contract.expected_model);
        assert_eq!(request.stream_requested, contract.expected_stream);
    }

    fn count_cache_controls(value: &Value) -> usize {
        match value {
            Value::Object(map) => {
                let current = if map.contains_key("cache_control") {
                    1
                } else {
                    0
                };
                current + map.values().map(count_cache_controls).sum::<usize>()
            }
            Value::Array(items) => items.iter().map(count_cache_controls).sum(),
            _ => 0,
        }
    }

    #[test]
    fn copilot_capabilities_are_native_for_fixture_verified_surfaces() {
        for (app, adapter) in [
            (AppKind::Claude, "claude_to_copilot_chat"),
            (AppKind::Codex, "codex_to_copilot_chat"),
            (AppKind::Gemini, "gemini_to_copilot_chat"),
        ] {
            let capability = capability_for(app, ProviderType::GitHubCopilot);
            assert_eq!(capability.adapter, adapter);
            assert_eq!(capability.support, AdapterSupport::Native);
            assert!(capability.requires_transform);
            assert!(capability.supports_stream_usage);
            assert!(capability.supports_model_list);
        }
    }

    #[test]
    fn capabilities_mark_same_format_api_key_adapters_as_native() {
        let claude = capability_for(AppKind::Claude, ProviderType::Claude);
        assert_eq!(claude.adapter, "claude_anthropic_api");
        assert_eq!(claude.support, AdapterSupport::Native);
        assert!(!claude.requires_transform);
        assert!(claude.supports_stream_usage);

        let codex = capability_for(AppKind::Codex, ProviderType::Codex);
        assert_eq!(codex.adapter, "codex_openai_compatible");
        assert_eq!(codex.support, AdapterSupport::Native);

        let gemini = capability_for(AppKind::Gemini, ProviderType::Gemini);
        assert_eq!(gemini.adapter, "gemini_api_key");
        assert_eq!(gemini.support, AdapterSupport::Native);
    }

    #[test]
    fn gemini_api_key_adapter_contract_uses_native_endpoint_and_key_header() {
        let stored = StoredProvider {
            app: AppKind::Gemini,
            provider: crate::domain::providers::model::Provider {
                id: "gemini-1".to_string(),
                name: "Gemini".to_string(),
                settings_config: json!({
                    "env": {
                        "GEMINI_BASE_URL": "https://generativelanguage.googleapis.com",
                        "GEMINI_API_KEY": "secret"
                    }
                }),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            provider_type: ProviderType::Gemini,
            provider_type_id: "gemini".to_string(),
            resource: Default::default(),
        };

        assert_adapter_contract(AdapterContract {
            app: AppKind::Gemini,
            provider_type: ProviderType::Gemini,
            route: ProxyRoute::Gemini,
            gemini_path: Some("models/gemini-2.5-pro:generateContent".to_string()),
            stored,
            request_body: br#"{"model":"gemini-2.5-pro","stream":true}"#,
            expected_endpoint:
                "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:generateContent",
            expected_header: ("x-goog-api-key", "secret"),
            expected_model: Some("gemini-2.5-pro"),
            expected_stream: true,
        });
        assert!(capability_for(AppKind::Gemini, ProviderType::Gemini).supports_stream_usage);
    }

    #[test]
    fn gemini_openai_compatible_capabilities_expose_stream_usage_after_stream_bridge() {
        for provider_type in [
            ProviderType::OpenRouter,
            ProviderType::OllamaCloud,
            ProviderType::Nvidia,
            ProviderType::DeepSeekApi,
        ] {
            let capability = capability_for(AppKind::Gemini, provider_type);
            assert_eq!(capability.support, AdapterSupport::Native);
            assert!(capability.requires_transform);
            assert!(capability.supports_stream_usage);
        }
    }

    #[test]
    fn cursor_capabilities_are_native_agentservice() {
        let cases = [
            (
                AppKind::Claude,
                ProviderType::CursorOAuth,
                "claude_cursor_agentservice",
            ),
            (
                AppKind::Claude,
                ProviderType::CursorApiKey,
                "claude_cursor_apikey_agentservice",
            ),
            (
                AppKind::Codex,
                ProviderType::CursorOAuth,
                "codex_cursor_agentservice",
            ),
            (
                AppKind::Codex,
                ProviderType::CursorApiKey,
                "codex_cursor_apikey_agentservice",
            ),
            (
                AppKind::Gemini,
                ProviderType::CursorOAuth,
                "gemini_cursor_agentservice",
            ),
            (
                AppKind::Gemini,
                ProviderType::CursorApiKey,
                "gemini_cursor_agentservice",
            ),
        ];

        for (app, provider_type, adapter_name) in cases {
            let capability = capability_for(app, provider_type);
            assert_eq!(capability.adapter, adapter_name);
            assert_eq!(capability.support, AdapterSupport::Native);
            assert!(capability.requires_transform);
            assert!(!capability.supports_stream_usage);
            assert!(!capability.supports_model_list);
        }
    }

    #[test]
    fn kiro_capabilities_are_native_for_claude_and_codex_only() {
        for (app, adapter) in [
            (AppKind::Claude, "claude_kiro_codewhisperer"),
            (AppKind::Codex, "codex_to_kiro_anthropic"),
        ] {
            let capability = capability_for(app, ProviderType::KiroOAuth);
            assert_eq!(capability.adapter, adapter);
            assert_eq!(capability.support, AdapterSupport::Native);
            assert!(capability.requires_transform);
            assert!(capability.supports_stream_usage);
            assert!(capability.supports_model_list);
        }

        let gemini = capability_for(AppKind::Gemini, ProviderType::KiroOAuth);
        assert_eq!(gemini.adapter, "gemini_kiro_skeleton");
        assert_eq!(gemini.support, AdapterSupport::GenericFallback);
        assert!(!gemini.requires_transform);
        assert!(!gemini.supports_stream_usage);
        assert!(!gemini.supports_model_list);
    }

    #[test]
    fn claude_deepseek_account_capability_is_planned_with_stream_usage() {
        let capability = capability_for(AppKind::Claude, ProviderType::DeepSeekAccount);
        assert_eq!(capability.adapter, "claude_deepseek_account_planned");
        assert_eq!(capability.support, AdapterSupport::Planned);
        assert!(capability.requires_transform);
        assert!(capability.supports_stream_usage);
    }

    #[test]
    fn unverified_account_and_cross_protocol_skeletons_remain_fallbacks() {
        let cases = [
            (
                AppKind::Codex,
                ProviderType::DeepSeekAccount,
                "codex_deepseek_skeleton",
            ),
            (
                AppKind::Gemini,
                ProviderType::DeepSeekAccount,
                "gemini_deepseek_skeleton",
            ),
            (
                AppKind::Gemini,
                ProviderType::KiroOAuth,
                "gemini_kiro_skeleton",
            ),
        ];

        for (app, provider_type, adapter_name) in cases {
            let capability = capability_for(app, provider_type);
            assert_eq!(capability.adapter, adapter_name);
            assert_eq!(capability.support, AdapterSupport::GenericFallback);
        }
    }

    #[test]
    fn claude_copilot_managed_preflight_uses_chat_endpoint_and_optimizer_headers() {
        let adapter = adapter_for(AppKind::Claude, ProviderType::GitHubCopilot);
        let mut stored = stored_provider(
            AppKind::Claude,
            ProviderType::GitHubCopilot,
            json!({
                "modelCatalog": {
                    "models": [
                        {"id": "claude-sonnet-4.6"},
                        {"id": "claude-sonnet-4.6-1m"}
                    ]
                }
            }),
        );
        bind_managed_test_provider(&mut stored, "copilot-account");
        let accounts = AccountStore {
            accounts: vec![managed_test_account(
                ProviderType::GitHubCopilot,
                "copilot-account",
                "copilot-token",
                json!({}),
            )],
            ..Default::default()
        };
        let headers = adapter
            .build_headers(AppKind::Claude, &stored, &accounts)
            .unwrap();
        assert!(headers
            .iter()
            .any(|item| item == &("authorization", "Bearer copilot-token".to_string())));
        assert!(!headers.iter().any(|(name, _)| *name == "anthropic-version"));

        let metadata = CopilotRequestMetadata {
            has_anthropic_beta: false,
            session_id: Some("session-1".to_string()),
        };
        let request = adapter
            .transform_request_for_route_with_metadata(
                Bytes::from_static(
                    br#"{"model":"claude-sonnet-4-6[1m]","messages":[{"role":"assistant","content":[{"type":"tool_use","id":"tool_1","name":"Read","input":{}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool_1","content":"ok"},{"type":"text","text":"continue"}]}],"stream":true}"#,
                ),
                &stored,
                ProxyRoute::ClaudeMessages,
                None,
                &metadata,
            )
            .unwrap();
        let endpoint = adapter
            .resolve_endpoint_for_request(ProxyRoute::ClaudeMessages, None, &stored, &request)
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(endpoint, "https://api.githubcopilot.com/chat/completions");
        assert_eq!(
            request.actual_model.as_deref(),
            Some("claude-sonnet-4.6-1m")
        );
        assert_eq!(
            request.actual_model_source.as_deref(),
            Some("copilot_model_normalization")
        );
        assert_eq!(
            value.get("model").and_then(Value::as_str),
            Some("claude-sonnet-4.6-1m")
        );
        assert!(request
            .upstream_headers
            .iter()
            .any(|item| item == &("x-initiator", "agent".to_string())));
        assert!(request
            .upstream_headers
            .iter()
            .any(|(name, _)| *name == "x-interaction-id"));
        assert!(request.stream_requested);
    }

    #[test]
    fn copilot_preflight_cannot_override_authoritative_single_model() {
        let adapter = adapter_for(AppKind::Claude, ProviderType::GitHubCopilot);
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::GitHubCopilot,
            json!({
                "env": {"ANTHROPIC_AUTH_TOKEN": "copilot-token"},
                "modelCatalog": {
                    "models": [{"id": "claude-sonnet-4.6-1m"}]
                },
                "modelMapping": {
                    "mode": "single",
                    "upstreamModel": "claude-sonnet-5"
                }
            }),
        );
        let request = adapter
            .transform_request_for_route_with_metadata(
                Bytes::from_static(
                    br#"{"model":"claude-sonnet-4-6[1m]","messages":[{"role":"user","content":"ping"}],"stream":false}"#,
                ),
                &stored,
                ProxyRoute::ClaudeMessages,
                None,
                &CopilotRequestMetadata::default(),
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(
            request.requested_model.as_deref(),
            Some("claude-sonnet-4-6[1m]")
        );
        assert_eq!(request.actual_model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(
            request.actual_model_source.as_deref(),
            Some("model_mapping_single")
        );
        assert_eq!(
            value.get("model").and_then(Value::as_str),
            Some("claude-sonnet-5")
        );
    }

    #[test]
    fn cross_protocol_static_contracts_are_native_when_transform_and_endpoint_are_closed() {
        let cases = [
            (
                AppKind::Claude,
                ProviderType::Codex,
                "claude_to_codex_responses",
                true,
            ),
            (
                AppKind::Claude,
                ProviderType::GeminiCli,
                "claude_to_gemini_native",
                true,
            ),
            (
                AppKind::Claude,
                ProviderType::AntigravityOAuth,
                "claude_antigravity_gemini_native",
                true,
            ),
            (
                AppKind::Codex,
                ProviderType::ClaudeOAuth,
                "codex_to_claude_messages",
                true,
            ),
            (
                AppKind::Codex,
                ProviderType::GeminiCli,
                "codex_to_gemini_native",
                true,
            ),
            (
                AppKind::Codex,
                ProviderType::AntigravityOAuth,
                "codex_antigravity_gemini_native",
                true,
            ),
            (
                AppKind::Gemini,
                ProviderType::AntigravityOAuth,
                "gemini_antigravity_native",
                false,
            ),
            (
                AppKind::Gemini,
                ProviderType::OllamaCloud,
                "gemini_ollama_openai_chat",
                true,
            ),
            (
                AppKind::Gemini,
                ProviderType::ClaudeOAuth,
                "gemini_to_claude_messages",
                true,
            ),
            (
                AppKind::Gemini,
                ProviderType::CodexOAuth,
                "gemini_to_codex_responses",
                true,
            ),
        ];

        for (app, provider_type, adapter, requires_transform) in cases {
            let capability = capability_for(app, provider_type);
            assert_eq!(capability.adapter, adapter);
            assert_eq!(capability.support, AdapterSupport::Native);
            assert_eq!(capability.requires_transform, requires_transform);
            assert!(capability.supports_stream_usage);
            assert!(!capability.supports_oauth_refresh);
        }
    }

    fn gemini_v1internal_runtime_plan(
        provider_type: ProviderType,
        account_id: &str,
    ) -> ProviderRuntimePlan {
        let (profile_id, driver_id, driver_contract_revision, endpoint, upstream_protocol) =
            match provider_type {
                ProviderType::GeminiCli => (
                    "gemini.google_oauth",
                    "oauth.gemini_code_assist",
                    1,
                    "https://cloudcode-pa.googleapis.com",
                    UpstreamProtocol::GeminiNative,
                ),
                ProviderType::AntigravityOAuth => (
                    "gemini.antigravity_oauth",
                    "special.antigravity",
                    2,
                    "https://daily-cloudcode-pa.googleapis.com",
                    UpstreamProtocol::Special,
                ),
                ProviderType::AgyOAuth => (
                    "gemini.antigravity_cli",
                    "special.agy",
                    2,
                    "https://daily-cloudcode-pa.googleapis.com",
                    UpstreamProtocol::Special,
                ),
                _ => unreachable!(),
            };
        let outbound_identity_policy = match provider_type {
            ProviderType::GeminiCli => {
                crate::domain::providers::registry::OutboundIdentityPolicy::ManagedIdentity {
                    family: crate::domain::providers::registry::ManagedIdentityFamily::GeminiCli,
                }
            }
            ProviderType::AntigravityOAuth | ProviderType::AgyOAuth => {
                crate::domain::providers::registry::OutboundIdentityPolicy::ManagedVersion {
                    family: crate::domain::providers::registry::ManagedVersionFamily::Antigravity,
                }
            }
            _ => unreachable!(),
        };
        ProviderRuntimePlan {
            provider_key: crate::domain::providers::registry::ProviderKey::new(
                AppKind::Gemini,
                format!("{}-fixture", provider_type.as_str()),
            )
            .unwrap(),
            provider_revision: 1,
            profile_id: crate::domain::providers::registry::ProfileId::parse(profile_id).unwrap(),
            profile_schema_revision: 1,
            driver_id: crate::domain::providers::registry::DriverId::parse(driver_id).unwrap(),
            driver_contract_revision,
            endpoint: endpoint.to_string(),
            upstream_protocol,
            outbound_identity_policy,
            auth_ref: crate::domain::providers::runtime::RuntimeAuthRef::ManagedAccount {
                account_id: account_id.to_string(),
                expected_provider_type: provider_type,
                auth_identity_generation: 1,
            },
            model_policy: crate::domain::providers::runtime::RuntimeModelPolicy::Passthrough,
            coding_plan: None,
            test_model: None,
            probe_policy_fingerprint: "fixture".to_string(),
            aws_region: None,
            media_policy: None,
            transport_policy: Default::default(),
            extra_headers: Vec::new(),
            driver_options: Default::default(),
            configuration_state:
                crate::domain::providers::runtime::RuntimeConfigurationState::Ready,
            warnings: Vec::new(),
            runtime_fingerprint: "gemini-v1internal-fixture".to_string(),
        }
    }

    #[test]
    fn gemini_v1internal_providers_wrap_exact_bound_project_and_separate_identity() {
        for (provider_type, project_evidence, expected_project, antigravity_identity) in [
            (
                ProviderType::GeminiCli,
                json!({"profile": {"projectId": "gemini-cli-project"}}),
                "gemini-cli-project",
                false,
            ),
            (
                ProviderType::AntigravityOAuth,
                json!({"profile": {"projectId": "antigravity-project"}}),
                "antigravity-project",
                true,
            ),
            (
                ProviderType::AgyOAuth,
                json!({
                    "quota": {
                        "success": true,
                        "extraUsage": {
                            "loadCodeAssist": {
                                "cloudaicompanionProject": {"id": "agy-project"}
                            }
                        }
                    }
                }),
                "agy-project",
                true,
            ),
        ] {
            let account_id = format!("{}-account", provider_type.as_str());
            let mut account = json!({
                "id": account_id,
                "providerType": provider_type.as_str(),
                "authIdentityGeneration": 1,
                "accessToken": "access-token"
            });
            for (key, value) in project_evidence.as_object().unwrap() {
                account[key] = value.clone();
            }
            let accounts = AccountStore {
                accounts: vec![serde_json::from_value(account).unwrap()],
                ..Default::default()
            };
            let stored = stored_provider(AppKind::Gemini, provider_type, json!({}));
            let plan = gemini_v1internal_runtime_plan(provider_type, &account_id);
            let adapter = adapter_for(AppKind::Gemini, provider_type);
            let mut request = adapter
                .transform_request_for_route(
                    Bytes::from_static(
                        br#"{"contents":[{"role":"user","parts":[{"text":"ping"}]}],"model":"client-model","project":"client-project","stream":false,"safetySettings":[{}]}"#,
                    ),
                    &stored,
                    ProxyRoute::Gemini,
                    Some("models/gemini-3.5-flash-medium:generateContent"),
                )
                .unwrap();
            request.actual_model = Some("gemini-3.5-flash-medium".to_string());
            assert!(!request.stream_requested);
            assert!(request.upstream_stream_requested);

            let mut endpoint = resolve_runtime_endpoint_for_request(
                &plan,
                ProxyRoute::Gemini,
                Some("models/gemini-3.5-flash-medium:generateContent".to_string()),
                &stored,
                &request,
            )
            .unwrap();
            let mut headers = Vec::new();
            finalize_runtime_protocol_auth(
                &plan,
                &stored,
                &accounts,
                &mut request,
                &mut endpoint,
                &mut headers,
            )
            .unwrap();

            let expected_base = if antigravity_identity {
                ANTIGRAVITY_PRODUCTION_BASE_URL
            } else {
                plan.endpoint.as_str()
            };
            assert_eq!(endpoint, gemini_v1internal_endpoint(expected_base));
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["project"], expected_project);
            assert_eq!(body["model"], "gemini-3.5-flash-medium");
            assert!(body.pointer("/request/model").is_none());
            assert!(body.pointer("/request/project").is_none());
            assert!(body.pointer("/request/stream").is_none());
            if antigravity_identity {
                assert_eq!(body["userAgent"], "antigravity");
                assert_eq!(body["requestType"], "agent");
                assert!(body["requestId"]
                    .as_str()
                    .is_some_and(|value| value.starts_with("agent-") && value.len() == 42));
                assert!(body.pointer("/request/safetySettings").is_none());
                assert!(body
                    .pointer("/request/systemInstruction/parts/0/text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.contains("You are Antigravity")));
            } else {
                assert_eq!(body.as_object().unwrap().len(), 3);
                assert!(body.get("requestId").is_none());
                assert!(body.get("userAgent").is_none());
                assert!(body.get("requestType").is_none());
                assert_eq!(body.pointer("/request/safetySettings"), Some(&json!([{}])));
                assert!(body.pointer("/request/systemInstruction").is_none());
            }
        }
    }

    #[test]
    fn gemini_v1internal_count_tokens_uses_direct_ai_studio_contract() {
        let account_id = "gemini-count-tokens-account";
        let accounts = AccountStore {
            accounts: vec![serde_json::from_value(json!({
                "id": account_id,
                "providerType": "gemini_cli",
                "accessToken": "access-token",
                "raw": {"testGeminiAiStudioBaseUrl": "http://127.0.0.1:43210"}
            }))
            .unwrap()],
            ..Default::default()
        };
        let stored = stored_provider(AppKind::Gemini, ProviderType::GeminiCli, json!({}));
        let plan = gemini_v1internal_runtime_plan(ProviderType::GeminiCli, account_id);
        let adapter = adapter_for(AppKind::Gemini, ProviderType::GeminiCli);
        let mut request = adapter
            .transform_request_for_route(
                Bytes::from_static(
                    br#"{"contents":[{"parts":[{"text":"count me"}]}],"model":"client-model","project":"client-project","stream":true}"#,
                ),
                &stored,
                ProxyRoute::Gemini,
                Some("models/gemini-2.5-pro:countTokens"),
            )
            .unwrap();
        request.actual_model = Some("gemini-2.5-pro".to_string());
        assert!(request.is_gemini_count_tokens());
        assert!(!request.stream_requested);
        assert!(!request.upstream_stream_requested);

        let mut endpoint = resolve_runtime_endpoint_for_request(
            &plan,
            ProxyRoute::Gemini,
            Some("models/gemini-2.5-pro:countTokens".to_string()),
            &stored,
            &request,
        )
        .unwrap();
        finalize_runtime_protocol_auth(
            &plan,
            &stored,
            &accounts,
            &mut request,
            &mut endpoint,
            &mut Vec::new(),
        )
        .unwrap();

        assert_eq!(
            endpoint,
            "http://127.0.0.1:43210/v1beta/models/gemini-2.5-pro:countTokens"
        );
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(
            body.pointer("/contents/0/parts/0/text"),
            Some(&json!("count me"))
        );
        assert!(body.get("request").is_none());
        assert!(body.get("project").is_none());
        assert!(body.get("model").is_none());
        assert!(body.get("stream").is_none());
    }

    #[test]
    fn antigravity_web_search_uses_fallback_model_and_preserves_function_tools() {
        for provider_type in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
            let account_id = format!("{}-web-search-account", provider_type.as_str());
            let accounts = AccountStore {
                accounts: vec![serde_json::from_value(json!({
                    "id": account_id,
                    "providerType": provider_type.as_str(),
                    "accessToken": "access-token",
                    "profile": {"projectId": "web-search-project"}
                }))
                .unwrap()],
                ..Default::default()
            };
            let stored = stored_provider(AppKind::Claude, provider_type, json!({}));
            let plan = gemini_v1internal_runtime_plan(provider_type, &account_id);
            let adapter = adapter_for(AppKind::Claude, provider_type);
            let mut request = adapter
                .transform_request_for_route(
                    Bytes::from_static(
                        br#"{"model":"claude-sonnet-4-6","messages":[{"role":"user","content":"search"}],"tools":[{"name":"lookup","description":"Lookup","input_schema":{"type":"object"}},{"type":"web_search_20250305","name":"web_search"}]}"#,
                    ),
                    &stored,
                    ProxyRoute::ClaudeMessages,
                    None,
                )
                .unwrap();
            request.actual_model = Some("claude-sonnet-4-6".to_string());
            let mut endpoint = plan.endpoint.clone();
            finalize_runtime_protocol_auth(
                &plan,
                &stored,
                &accounts,
                &mut request,
                &mut endpoint,
                &mut Vec::new(),
            )
            .unwrap();

            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["requestType"], "web_search");
            assert_eq!(body["model"], ANTIGRAVITY_WEB_SEARCH_FALLBACK_MODEL);
            assert_eq!(
                request.actual_model.as_deref(),
                Some(ANTIGRAVITY_WEB_SEARCH_FALLBACK_MODEL)
            );
            assert_eq!(
                request.actual_model_source.as_deref(),
                Some("antigravity_web_search_fallback")
            );
            assert_eq!(
                body.pointer("/request/tools/0/functionDeclarations/0/name"),
                Some(&json!("lookup"))
            );
            assert!(body.pointer("/request/tools/1/googleSearch").is_some());
            assert_eq!(body["request"]["tools"].as_array().unwrap().len(), 2);
        }
    }

    #[test]
    fn agy_v1internal_never_falls_back_to_antigravity_account_identity() {
        let stored = stored_provider(AppKind::Gemini, ProviderType::AgyOAuth, json!({}));
        let plan = gemini_v1internal_runtime_plan(ProviderType::AgyOAuth, "shared-account-id");
        let accounts = AccountStore {
            accounts: vec![serde_json::from_value(json!({
                "id": "shared-account-id",
                "providerType": "antigravity_oauth",
                "accessToken": "wrong-token",
                "profile": {"projectId": "wrong-project"}
            }))
            .unwrap()],
            ..Default::default()
        };
        let mut request = AdapterRequest {
            body: Bytes::from_static(br#"{"contents":[]}"#),
            upstream_endpoint: None,
            upstream_headers: Vec::new(),
            model: Some("gemini-3.5-flash-medium".to_string()),
            requested_model: Some("gemini-3.5-flash-medium".to_string()),
            actual_model: Some("gemini-3.5-flash-medium".to_string()),
            actual_model_source: None,
            gemini_action: None,
            stream_requested: false,
            upstream_stream_requested: true,
            custom_tool_names: Default::default(),
            responses_tool_context: Default::default(),
            claude_tool_name_map: Default::default(),
        };
        let mut endpoint = plan.endpoint.clone();
        let error = finalize_runtime_protocol_auth(
            &plan,
            &stored,
            &accounts,
            &mut request,
            &mut endpoint,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(error
            .message
            .contains("agy_oauth account shared-account-id"));
        assert!(error.message.contains("unavailable"));
    }

    #[test]
    fn antigravity_identity_patch_is_idempotent() {
        let mut request = json!({
            "systemInstruction": {
                "parts": [
                    {"text": "You are Antigravity. Existing official identity."},
                    {"text": "Keep this user system instruction."}
                ]
            },
            "contents": []
        });
        sanitize_gemini_v1internal_request(&mut request, true).unwrap();
        sanitize_gemini_v1internal_request(&mut request, true).unwrap();
        assert_eq!(
            request
                .pointer("/systemInstruction/parts")
                .and_then(Value::as_array)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn antigravity_mixed_tools_enable_server_side_invocations_without_losing_config() {
        let mut request = json!({
            "contents": [],
            "tools": [
                {"functionDeclarations": [{"name": "lookup", "parameters": {"type": "object"}}]},
                {"googleSearch": {}}
            ],
            "toolConfig": {
                "functionCallingConfig": {"mode": "AUTO"},
                "include_server_side_tool_invocations": false
            }
        });
        sanitize_gemini_v1internal_request(&mut request, true).unwrap();

        assert_eq!(
            request.pointer("/toolConfig/includeServerSideToolInvocations"),
            Some(&json!(true))
        );
        assert_eq!(
            request.pointer("/toolConfig/functionCallingConfig/mode"),
            Some(&json!("AUTO"))
        );
        assert!(request
            .pointer("/toolConfig/include_server_side_tool_invocations")
            .is_none());
    }

    #[test]
    fn antigravity_single_tool_kind_does_not_invent_mixed_tool_config() {
        for tools in [
            json!([{"functionDeclarations": [{"name": "lookup"}]}]),
            json!([{"googleSearch": {}}]),
        ] {
            let mut request = json!({"contents": [], "tools": tools});
            sanitize_gemini_v1internal_request(&mut request, true).unwrap();
            assert!(request.get("toolConfig").is_none());
        }
    }

    #[test]
    fn antigravity_endpoint_uses_paid_tier_authority_and_preserves_test_origins() {
        let account = |profile: Value, subscription_level: Option<&str>| Account {
            id: "antigravity-endpoint-account".to_string(),
            provider_type: ProviderType::AntigravityOAuth,
            profile: Some(profile),
            subscription_level: subscription_level.map(str::to_string),
            ..serde_json::from_value(json!({
                "id": "antigravity-endpoint-account",
                "providerType": "antigravity_oauth"
            }))
            .unwrap()
        };

        let mut unknown = gemini_v1internal_endpoint(ANTIGRAVITY_DAILY_BASE_URL);
        select_antigravity_inference_endpoint(&mut unknown, &account(json!({}), None));
        assert_eq!(
            unknown,
            gemini_v1internal_endpoint(ANTIGRAVITY_PRODUCTION_BASE_URL)
        );

        for paid in [
            account(json!({}), Some("Pro")),
            account(
                json!({"selectedTier": {"name": "custom-paid"}, "selectedTierSource": "paidTier"}),
                None,
            ),
        ] {
            let mut endpoint = gemini_v1internal_endpoint(ANTIGRAVITY_PRODUCTION_BASE_URL);
            select_antigravity_inference_endpoint(&mut endpoint, &paid);
            assert_eq!(
                endpoint,
                gemini_v1internal_endpoint(ANTIGRAVITY_DAILY_BASE_URL)
            );
        }

        let mut test_endpoint =
            "http://127.0.0.1:43210/v1internal:streamGenerateContent?alt=sse".to_string();
        select_antigravity_inference_endpoint(
            &mut test_endpoint,
            &account(json!({}), Some("Ultra")),
        );
        assert_eq!(
            test_endpoint,
            "http://127.0.0.1:43210/v1internal:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn gemini_v1internal_response_wrapper_is_removed_for_all_account_types() {
        for provider_type in [
            ProviderType::GeminiCli,
            ProviderType::AntigravityOAuth,
            ProviderType::AgyOAuth,
        ] {
            let stored = stored_provider(AppKind::Gemini, provider_type, json!({}));
            let adapter = adapter_for(AppKind::Gemini, provider_type);
            let response = adapter
                .transform_response(
                    Bytes::from_static(
                        br#"{"response":{"candidates":[{"content":{"role":"model","parts":[{"text":"pong"}]}}],"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":1}}}"#,
                    ),
                    &stored,
                    ProxyRoute::Gemini,
                )
                .unwrap();
            let response: Value = serde_json::from_slice(&response).unwrap();
            assert_eq!(
                response.pointer("/candidates/0/content/parts/0/text"),
                Some(&json!("pong")),
                "provider_type={}",
                provider_type.as_str()
            );
            assert!(response.get("response").is_none());
        }
    }

    #[test]
    fn gemini_v1internal_response_preserves_wrapper_metadata_without_overwriting_inner_values() {
        let response = unwrap_gemini_v1internal_value(json!({
            "responseId": "wrapper-response-id",
            "modelVersion": "wrapper-model",
            "response": {
                "responseId": "inner-response-id",
                "candidates": [{"finishReason": "STOP"}]
            }
        }));

        assert_eq!(response["responseId"], "inner-response-id");
        assert_eq!(response["modelVersion"], "wrapper-model");
        assert_eq!(response["candidates"][0]["finishReason"], "STOP");
        assert!(response.get("response").is_none());
    }

    #[test]
    fn gemini_v1internal_blocked_prompt_transforms_to_claude_refusal() {
        let stored = stored_provider(AppKind::Claude, ProviderType::AntigravityOAuth, json!({}));
        let adapter = adapter_for(AppKind::Claude, ProviderType::AntigravityOAuth);
        let response = adapter
            .transform_response(
                Bytes::from_static(
                    br#"{"response":{"responseId":"blocked","modelVersion":"gemini-2.5-flash","promptFeedback":{"blockReason":"SAFETY"},"usageMetadata":{"promptTokenCount":4,"candidatesTokenCount":0}}}"#,
                ),
                &stored,
                ProxyRoute::ClaudeMessages,
            )
            .unwrap();
        let response: Value = serde_json::from_slice(&response).unwrap();
        assert_eq!(response["type"], "message");
        assert_eq!(response["content"], json!([]));
        assert_eq!(response["stop_reason"], "refusal");
        assert_eq!(response["usage"]["input_tokens"], 4);
    }

    #[test]
    fn gemini_v1internal_candidate_safety_transforms_to_claude_refusal() {
        let stored = stored_provider(AppKind::Claude, ProviderType::AntigravityOAuth, json!({}));
        let adapter = adapter_for(AppKind::Claude, ProviderType::AntigravityOAuth);
        let response = adapter
            .transform_response(
                Bytes::from_static(
                    br#"{"response":{"responseId":"candidate-blocked","modelVersion":"gemini-2.5-flash","candidates":[{"finishReason":"SAFETY"}],"usageMetadata":{"promptTokenCount":4,"candidatesTokenCount":0}}}"#,
                ),
                &stored,
                ProxyRoute::ClaudeMessages,
            )
            .unwrap();
        let response: Value = serde_json::from_slice(&response).unwrap();
        assert_eq!(response["type"], "message");
        assert_eq!(response["content"], json!([]));
        assert_eq!(response["stop_reason"], "refusal");
    }

    #[test]
    fn managed_gemini_bridge_fails_closed_on_malformed_success_payload() {
        let stored = stored_provider(AppKind::Claude, ProviderType::AntigravityOAuth, json!({}));
        let adapter = adapter_for(AppKind::Claude, ProviderType::AntigravityOAuth);
        let error = adapter
            .transform_response(
                Bytes::from_static(
                    br#"{"response":{"candidates":[{"content":{"parts":{}},"finishReason":"STOP"}]}}"#,
                ),
                &stored,
                ProxyRoute::ClaudeMessages,
            )
            .unwrap_err();

        assert_eq!(error.status, axum::http::StatusCode::BAD_GATEWAY);
        assert!(error.message.contains("response bridge failed"));
    }

    #[test]
    fn official_oauth_static_native_adapters_do_not_enable_refresh() {
        let cases = [
            (
                AppKind::Claude,
                ProviderType::ClaudeOAuth,
                "claude_oauth_bearer_compatible",
                false,
            ),
            (
                AppKind::Claude,
                ProviderType::CodexOAuth,
                "claude_to_codex_oauth_responses",
                true,
            ),
            (
                AppKind::Codex,
                ProviderType::CodexOAuth,
                "codex_oauth_responses",
                true,
            ),
            (
                AppKind::Gemini,
                ProviderType::GeminiCli,
                "gemini_cli_oauth_native",
                false,
            ),
        ];

        for (app, provider_type, adapter, requires_transform) in cases {
            let capability = capability_for(app, provider_type);
            assert_eq!(capability.adapter, adapter);
            assert_eq!(capability.support, AdapterSupport::Native);
            assert_eq!(capability.requires_transform, requires_transform);
            assert!(capability.supports_stream_usage);
            assert!(!capability.supports_oauth_refresh);
        }
    }

    #[test]
    fn bedrock_adapters_remain_planned_until_real_sigv4_forwarding_exists() {
        for (app, adapter_name) in [
            (AppKind::Claude, "claude_bedrock_signature_planned"),
            (AppKind::Codex, "codex_bedrock_planned"),
            (AppKind::Gemini, "gemini_bedrock_planned"),
        ] {
            let capability = capability_for(app, ProviderType::AwsBedrock);
            assert_eq!(capability.adapter, adapter_name);
            assert_eq!(capability.support, AdapterSupport::Planned);
            assert!(!capability.supports_stream_usage);
        }
    }

    #[test]
    fn exposes_all_provider_type_capabilities_for_each_app() {
        let capabilities = all_capabilities();
        assert_eq!(capabilities.len(), 66);
        assert!(capabilities.iter().any(|item| {
            item.app == AppKind::Gemini && item.provider_type == ProviderType::AntigravityOAuth
        }));
        assert!(capabilities.iter().any(|item| {
            item.app == AppKind::Codex && item.provider_type == ProviderType::GrokOAuth
        }));
        for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
            assert!(capabilities.iter().any(|item| {
                item.app == app
                    && item.provider_type == ProviderType::QoderCosy
                    && item.support == AdapterSupport::Native
            }));
        }
    }

    #[test]
    fn grok_capability_declares_responses_item_and_tool_boundaries() {
        let capability = capability_for(AppKind::Codex, ProviderType::GrokOAuth);
        let responses = capability
            .upstream_responses_protocol
            .expect("Grok must publish its Responses protocol boundary");

        assert_eq!(responses.endpoint, "/v1/responses");
        assert_eq!(responses.provider_protocol, "grok_responses");
        assert!(responses.accepted_input_item_types.contains(&"message"));
        assert!(responses
            .accepted_input_item_types
            .contains(&"shell_call_output"));
        assert!(responses
            .adapted_input_item_types
            .contains(&"additional_tools"));
        assert!(responses
            .accepted_top_level_tool_types
            .contains(&"function"));
        assert!(!responses.accepted_top_level_tool_types.contains(&"custom"));
        assert_eq!(
            responses.additional_tools_policy,
            crate::proxy::protocol_compat::AdditionalToolsPolicy::LosslessTopLevelMergeOrReject
        );
        assert_eq!(
            responses.unknown_input_item_policy,
            crate::proxy::protocol_compat::UnknownInputItemPolicy::Reject
        );
    }

    #[test]
    fn claude_app_codex_provider_transforms_anthropic_to_openai_responses() {
        let adapter = adapter_for(AppKind::Claude, ProviderType::Codex);
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::Codex,
            json!({
                "env": {
                    "OPENAI_BASE_URL": "https://api.example",
                    "OPENAI_API_KEY": "secret"
                },
                "modelMapping": {
                    "upstreamModel": "gpt-5.5"
                }
            }),
        );

        let headers = adapter
            .build_headers(AppKind::Claude, &stored, &AccountStore::default())
            .unwrap();
        assert!(headers
            .iter()
            .any(|item| item == &("authorization", "Bearer secret".to_string())));
        assert!(!headers.iter().any(|(name, _)| *name == "x-api-key"));

        let request = adapter
            .transform_request(
                Bytes::from_static(
                    br#"{"model":"claude-sonnet-4","system":"s","messages":[{"role":"user","content":[{"type":"text","text":"hello"}]}],"stream":true}"#,
                ),
                &stored,
            )
            .unwrap();
        let endpoint = adapter
            .resolve_endpoint_for_request(ProxyRoute::ClaudeMessages, None, &stored, &request)
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(endpoint, "https://api.example/v1/responses");
        assert_eq!(request.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(value.get("model").and_then(Value::as_str), Some("gpt-5.5"));
        assert!(value.get("instructions").is_none());
        assert_eq!(
            value.pointer("/input/0/role").and_then(Value::as_str),
            Some("system")
        );
        assert_eq!(
            value.pointer("/input/0/content").and_then(Value::as_str),
            Some("s")
        );
        assert_eq!(
            value
                .pointer("/input/1/content/0/type")
                .and_then(Value::as_str),
            Some("input_text")
        );
    }

    #[test]
    fn claude_app_gemini_provider_transforms_anthropic_to_gemini_native() {
        let adapter = adapter_for(AppKind::Claude, ProviderType::Gemini);
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::Gemini,
            json!({
                "env": {
                    "GEMINI_BASE_URL": "https://generativelanguage.googleapis.com",
                    "GEMINI_API_KEY": "secret"
                },
                "modelMapping": {
                    "upstreamModel": "gemini-2.5-pro"
                }
            }),
        );

        let request = adapter
            .transform_request(
                Bytes::from_static(
                    br#"{"model":"claude-sonnet-4","system":"s","messages":[{"role":"user","content":[{"type":"text","text":"hello"}]}],"stream":true}"#,
                ),
                &stored,
            )
            .unwrap();
        let endpoint = adapter
            .resolve_endpoint_for_request(ProxyRoute::ClaudeMessages, None, &stored, &request)
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(
            endpoint,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:streamGenerateContent"
        );
        assert!(request.stream_requested);
        assert_eq!(
            value
                .pointer("/systemInstruction/parts/0/text")
                .and_then(Value::as_str),
            Some("s")
        );
        assert_eq!(
            value
                .pointer("/contents/0/parts/0/text")
                .and_then(Value::as_str),
            Some("hello")
        );
    }

    #[test]
    fn codex_app_claude_provider_transforms_openai_to_anthropic() {
        let adapter = adapter_for(AppKind::Codex, ProviderType::Claude);
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::Claude,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://api.anthropic.com",
                    "ANTHROPIC_API_KEY": "secret"
                },
                "modelMapping": {
                    "upstreamModel": "claude-sonnet-4"
                }
            }),
        );

        let request = adapter
            .transform_request(
                Bytes::from_static(
                    br#"{"model":"gpt-5.5","input":[{"role":"user","content":[{"type":"input_text","text":"ping"}]}],"stream":true}"#,
                ),
                &stored,
            )
            .unwrap();
        let endpoint = adapter
            .resolve_endpoint_for_request(ProxyRoute::CodexResponses, None, &stored, &request)
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(endpoint, "https://api.anthropic.com/v1/messages");
        assert_eq!(request.model.as_deref(), Some("claude-sonnet-4"));
        assert_eq!(
            value
                .pointer("/messages/0/content/0/text")
                .and_then(Value::as_str),
            Some("ping")
        );
    }

    #[test]
    fn gemini_app_codex_provider_transforms_gemini_to_openai_responses() {
        let adapter = adapter_for(AppKind::Gemini, ProviderType::Codex);
        let stored = stored_provider(
            AppKind::Gemini,
            ProviderType::Codex,
            json!({
                "env": {
                    "OPENAI_BASE_URL": "https://api.example",
                    "OPENAI_API_KEY": "secret"
                },
                "modelMapping": {
                    "upstreamModel": "gpt-5.5"
                }
            }),
        );

        let request = adapter
            .transform_request(
                Bytes::from_static(
                    br#"{"model":"gemini-2.5-pro","systemInstruction":{"parts":[{"text":"system"}]},"contents":[{"role":"user","parts":[{"text":"hello"}]}],"stream":false}"#,
                ),
                &stored,
            )
            .unwrap();
        let endpoint = adapter
            .resolve_endpoint_for_request(
                ProxyRoute::Gemini,
                Some("models/gemini-2.5-pro:generateContent".to_string()),
                &stored,
                &request,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(endpoint, "https://api.example/v1/responses");
        assert_eq!(request.model.as_deref(), Some("gpt-5.5"));
        assert!(value.get("instructions").is_none());
        assert_eq!(
            value.pointer("/input/0/role").and_then(Value::as_str),
            Some("system")
        );
        assert_eq!(
            value.pointer("/input/0/content").and_then(Value::as_str),
            Some("system")
        );
        assert_eq!(
            value
                .pointer("/input/1/content/0/text")
                .and_then(Value::as_str),
            Some("hello")
        );
    }

    #[test]
    fn kimi_selects_native_messages_only_for_the_claude_source_surface() {
        let claude = stored_provider(
            AppKind::Claude,
            ProviderType::KimiCode,
            json!({"modelMapping":{"mode":"single","upstreamModel":"kimi-for-coding"}}),
        );
        let request = adapter_for(AppKind::Claude, ProviderType::KimiCode)
            .transform_request_for_route(
                Bytes::from_static(
                    br#"{"model":"claude-sonnet-5","max_tokens":128,"messages":[{"role":"user","content":"ping"}],"stream":true}"#,
                ),
                &claude,
                ProxyRoute::ClaudeMessages,
                None,
            )
            .unwrap();
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let capability = capability_for(AppKind::Claude, ProviderType::KimiCode);

        assert_eq!(capability.adapter, "claude_kimi_messages");
        assert!(!capability.requires_transform);
        assert_eq!(body["messages"][0]["content"], "ping");
        assert!(body.get("input").is_none());
        assert_eq!(
            upstream_format_for_route(&claude, Some(ProxyRoute::ClaudeMessages), &request.body),
            Some(UpstreamFormat::AnthropicMessages)
        );

        for app in [AppKind::Codex, AppKind::Gemini] {
            let stored = stored_provider(app, ProviderType::KimiCode, json!({}));
            assert_eq!(
                upstream_format_for_route(&stored, None, &[]),
                Some(UpstreamFormat::OpenAiChat),
                "app={app:?}"
            );
        }
    }

    #[test]
    fn gemini_route_model_is_injected_before_cross_protocol_transforms() {
        for provider_type in [
            ProviderType::Claude,
            ProviderType::Codex,
            ProviderType::OpenRouter,
        ] {
            let stored = stored_provider(AppKind::Gemini, provider_type, json!({}));
            let request = adapter_for(AppKind::Gemini, provider_type)
                .transform_request_for_route(
                    Bytes::from_static(
                        br#"{"contents":[{"role":"user","parts":[{"text":"hello"}]}]}"#,
                    ),
                    &stored,
                    ProxyRoute::Gemini,
                    Some("models/gemini-2.5-pro:generateContent"),
                )
                .unwrap();
            let body: Value = serde_json::from_slice(&request.body).unwrap();

            assert_eq!(
                body["model"], "gemini-2.5-pro",
                "provider={provider_type:?}"
            );
            assert_eq!(request.requested_model.as_deref(), Some("gemini-2.5-pro"));
            assert_eq!(request.actual_model.as_deref(), Some("gemini-2.5-pro"));
            assert_eq!(request.actual_model_source.as_deref(), Some("request"));
        }
    }

    #[test]
    fn native_gemini_route_actions_do_not_inject_model_into_body() {
        let stored = stored_provider(AppKind::Gemini, ProviderType::Gemini, json!({}));
        let adapter = adapter_for(AppKind::Gemini, ProviderType::Gemini);

        for action in ["generateContent", "countTokens"] {
            let path = format!("models/gemini-2.5-pro:{action}");
            let request = adapter
                .transform_request_for_route(
                    Bytes::from_static(
                        br#"{"contents":[{"role":"user","parts":[{"text":"hello"}]}]}"#,
                    ),
                    &stored,
                    ProxyRoute::Gemini,
                    Some(&path),
                )
                .unwrap();
            let body: Value = serde_json::from_slice(&request.body).unwrap();

            assert!(body.get("model").is_none(), "action={action}");
            assert!(request.requested_model.is_none(), "action={action}");
            assert!(request.actual_model.is_none(), "action={action}");
        }
    }

    #[test]
    fn gemini_route_controls_survive_all_cross_protocol_transforms() {
        let input = Bytes::from_static(
            br#"{"contents":[{"role":"user","parts":[{"text":"hello"}]}],"generationConfig":{"maxOutputTokens":1024,"temperature":0.25,"topP":0.75,"topK":32,"stopSequences":["DONE"]},"tools":[{"functionDeclarations":[{"name":"lookup","parameters":{"type":"object"}},{"name":"ignored","parameters":{"type":"object"}}]}],"toolConfig":{"functionCallingConfig":{"mode":"ANY","allowedFunctionNames":["lookup"]}}}"#,
        );

        for provider_type in [
            ProviderType::Claude,
            ProviderType::Codex,
            ProviderType::OpenRouter,
        ] {
            let stored = stored_provider(AppKind::Gemini, provider_type, json!({}));
            let request = adapter_for(AppKind::Gemini, provider_type)
                .transform_request_for_route(
                    input.clone(),
                    &stored,
                    ProxyRoute::Gemini,
                    Some("models/gemini-2.5-pro:generateContent"),
                )
                .unwrap();
            let body: Value = serde_json::from_slice(&request.body).unwrap();

            assert_eq!(body["model"], "gemini-2.5-pro");
            assert_eq!(body["temperature"], json!(0.25));
            assert_eq!(body["top_p"], json!(0.75));
            assert_eq!(body["tools"].as_array().unwrap().len(), 1);
            match provider_type {
                ProviderType::Claude => {
                    assert_eq!(body["max_tokens"], 1024);
                    assert_eq!(body["top_k"], 32);
                    assert_eq!(body["stop_sequences"], json!(["DONE"]));
                    assert_eq!(
                        body["tool_choice"],
                        json!({"type": "tool", "name": "lookup"})
                    );
                }
                ProviderType::Codex => {
                    assert_eq!(body["max_output_tokens"], 1024);
                    assert_eq!(body["stop"], json!(["DONE"]));
                    assert_eq!(body["tool_choice"]["name"], "lookup");
                    assert_eq!(
                        body.pointer("/metadata/geminiGenerationConfig/topK"),
                        Some(&json!(32))
                    );
                    assert!(body.get("top_k").is_none());
                }
                ProviderType::OpenRouter => {
                    assert_eq!(body["max_tokens"], 1024);
                    assert_eq!(body["stop"], json!(["DONE"]));
                    assert_eq!(
                        body.pointer("/tool_choice/function/name"),
                        Some(&json!("lookup"))
                    );
                    assert_eq!(
                        body.pointer("/metadata/geminiGenerationConfig/topK"),
                        Some(&json!(32))
                    );
                    assert!(body.get("top_k").is_none());
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn gemini_safety_responses_bridge_legally_to_both_codex_routes() {
        let stored = stored_provider(AppKind::Codex, ProviderType::Gemini, json!({}));
        let adapter = adapter_for(AppKind::Codex, ProviderType::Gemini);
        let payloads = [
            Bytes::from_static(
                br#"{"responseId":"prompt-blocked","modelVersion":"gemini-2.5-flash","promptFeedback":{"blockReason":"SAFETY"}}"#,
            ),
            Bytes::from_static(
                br#"{"responseId":"candidate-blocked","modelVersion":"gemini-2.5-flash","candidates":[{"finishReason":"SAFETY"}]}"#,
            ),
        ];

        for payload in payloads {
            let chat = adapter
                .transform_response(payload.clone(), &stored, ProxyRoute::CodexChatCompletions)
                .unwrap();
            let chat: Value = serde_json::from_slice(&chat).unwrap();
            assert!(chat
                .pointer("/choices/0/message/content")
                .is_some_and(Value::is_null));
            assert!(chat
                .pointer("/choices/0/message/refusal")
                .and_then(Value::as_str)
                .is_some_and(|refusal| !refusal.is_empty()));
            assert_eq!(chat["choices"][0]["finish_reason"], "content_filter");

            let responses = adapter
                .transform_response(payload, &stored, ProxyRoute::CodexResponses)
                .unwrap();
            let responses: Value = serde_json::from_slice(&responses).unwrap();
            assert_eq!(responses["status"], "incomplete");
            assert_eq!(responses["incomplete_details"]["reason"], "content_filter");
            assert_eq!(responses["output"][0]["content"][0]["type"], "refusal");
            assert!(responses["output"][0]["content"][0]["refusal"]
                .as_str()
                .is_some_and(|refusal| !refusal.is_empty()));
        }
    }

    #[test]
    fn gemini_synthetic_tool_ids_survive_both_codex_response_bridges() {
        let stored = stored_provider(AppKind::Codex, ProviderType::Gemini, json!({}));
        let adapter = adapter_for(AppKind::Codex, ProviderType::Gemini);
        let payload = Bytes::from_static(
            br#"{"responseId":"parallel-tools","modelVersion":"gemini-2.5-flash","candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"lookup","args":{"q":"first"}}},{"functionCall":{"id":"gemini_call_1","name":"lookup","args":{"q":"explicit"}}},{"functionCall":{"name":"lookup","args":{"q":"second"}}}]},"finishReason":"STOP"}]}"#,
        );
        let expected_ids = json!(["gemini_call_0", "gemini_call_1", "gemini_call_2"]);

        let chat = adapter
            .transform_response(payload.clone(), &stored, ProxyRoute::CodexChatCompletions)
            .unwrap();
        let chat: Value = serde_json::from_slice(&chat).unwrap();
        assert_eq!(
            Value::Array(
                chat["choices"][0]["message"]["tool_calls"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|call| call["id"].clone())
                    .collect()
            ),
            expected_ids
        );

        let responses = adapter
            .transform_response(payload, &stored, ProxyRoute::CodexResponses)
            .unwrap();
        let responses: Value = serde_json::from_slice(&responses).unwrap();
        assert_eq!(
            Value::Array(
                responses["output"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|call| call["call_id"].clone())
                    .collect()
            ),
            expected_ids
        );
    }

    #[test]
    fn generic_adapter_prepares_forwarding_request() {
        let adapter = adapter_for(AppKind::Claude, ProviderType::Claude);
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::Claude,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://example.com",
                    "ANTHROPIC_API_KEY": "secret"
                },
                "modelMapping": {
                    "upstreamModel": "glm-5.2"
                }
            }),
        );

        let request = adapter
            .transform_request(
                Bytes::from_static(br#"{"model":"gpt-5.5","stream":true}"#),
                &stored,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(request.model.as_deref(), Some("glm-5.2"));
        assert_eq!(request.requested_model.as_deref(), Some("gpt-5.5"));
        assert_eq!(request.actual_model.as_deref(), Some("glm-5.2"));
        assert_eq!(
            request.actual_model_source.as_deref(),
            Some("model_mapping")
        );
        assert!(request.stream_requested);
        assert_eq!(value.get("model").and_then(Value::as_str), Some("glm-5.2"));
        assert_eq!(
            adapter
                .resolve_endpoint(ProxyRoute::ClaudeMessages, None, &stored)
                .unwrap(),
            "https://example.com/v1/messages"
        );
    }

    #[test]
    fn single_mode_overrides_every_legacy_mapping_layer() {
        let adapter = adapter_for(AppKind::Claude, ProviderType::OpenRouter);
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::OpenRouter,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://openrouter.ai/api",
                    "ANTHROPIC_MODEL": "env-model"
                },
                "modelCatalog": {
                    "models": [{
                        "model": "client-model",
                        "upstreamModel": "catalog-model"
                    }]
                },
                "modelMapping": {
                    "mode": "single",
                    "upstreamModel": "single-model",
                    "client-model": "direct-model",
                    "rules": [{
                        "match": "client-*",
                        "upstreamModel": "rule-model"
                    }]
                }
            }),
        );

        let request = adapter
            .transform_request(
                Bytes::from_static(br#"{"model":"client-model","messages":[],"stream":false}"#),
                &stored,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(
            value.get("model").and_then(Value::as_str),
            Some("single-model")
        );
        assert_eq!(request.requested_model.as_deref(), Some("client-model"));
        assert_eq!(request.actual_model.as_deref(), Some("single-model"));
        assert_eq!(
            request.actual_model_source.as_deref(),
            Some("model_mapping_single")
        );
    }

    #[test]
    fn single_mode_injects_model_when_request_omits_it() {
        let (body, selection) = apply_model_mapping(
            Bytes::from_static(br#"{"stream":false}"#),
            &json!({
                "modelMapping": {
                    "mode": "single",
                    "upstreamModel": "grok-4.5"
                }
            }),
            ModelMappingContext {
                app: AppKind::Codex,
                route: Some(ProxyRoute::CodexResponses),
                provider_type: ProviderType::GrokOAuth,
            },
        );
        let value: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(value.get("model").and_then(Value::as_str), Some("grok-4.5"));
        assert_eq!(selection.requested_model, None);
        assert_eq!(selection.actual_model.as_deref(), Some("grok-4.5"));
    }

    #[test]
    fn passthrough_mode_ignores_stale_legacy_mappings() {
        let (body, selection) = apply_model_mapping(
            Bytes::from_static(br#"{"model":"gpt-5.6-sol"}"#),
            &json!({
                "modelMapping": {
                    "mode": "passthrough",
                    "upstreamModel": "stale-model",
                    "gpt-5.6-sol": "stale-direct-model"
                },
                "modelCatalog": {
                    "models": [{
                        "model": "gpt-5.6-sol",
                        "upstreamModel": "stale-catalog-model"
                    }]
                }
            }),
            ModelMappingContext {
                app: AppKind::Codex,
                route: Some(ProxyRoute::CodexResponses),
                provider_type: ProviderType::CodexOAuth,
            },
        );
        let value: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(
            value.get("model").and_then(Value::as_str),
            Some("gpt-5.6-sol")
        );
        assert_eq!(selection.requested_model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(selection.actual_model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(
            selection.actual_model_source.as_deref(),
            Some("model_mapping_passthrough")
        );
    }

    #[test]
    fn model_mapping_direct_exact_rule_preserves_actual_model() {
        let adapter = adapter_for(AppKind::Claude, ProviderType::Claude);
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::Claude,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://example.com",
                    "ANTHROPIC_API_KEY": "secret"
                },
                "modelMapping": {
                    "claude-sonnet-4": {
                        "upstreamModel": "anthropic/sonnet-4",
                    },
                    "upstreamModel": "fallback-model"
                }
            }),
        );

        let request = adapter
            .transform_request(
                Bytes::from_static(br#"{"model":"claude-sonnet-4","stream":false}"#),
                &stored,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(
            value.get("model").and_then(Value::as_str),
            Some("anthropic/sonnet-4")
        );
        assert_eq!(request.requested_model.as_deref(), Some("claude-sonnet-4"));
        assert_eq!(request.actual_model.as_deref(), Some("anthropic/sonnet-4"));
        assert_eq!(
            request.actual_model_source.as_deref(),
            Some("model_mapping_direct")
        );
        assert_eq!(request.actual_model.as_deref(), Some("anthropic/sonnet-4"));
    }

    #[test]
    fn model_mapping_catalog_maps_gemini_route_model() {
        let adapter = adapter_for(AppKind::Gemini, ProviderType::Gemini);
        let stored = stored_provider(
            AppKind::Gemini,
            ProviderType::Gemini,
            json!({
                "env": {
                    "GEMINI_BASE_URL": "https://generativelanguage.googleapis.com",
                    "GEMINI_API_KEY": "secret"
                },
                "modelCatalog": {
                    "models": [{
                        "model": "gemini-2.5-pro",
                        "upstreamModel": "models/gemini-2.5-pro-preview",
                    }]
                }
            }),
        );

        let request = adapter
            .transform_request_for_route(
                Bytes::from_static(
                    br#"{"model":"gemini-2.5-pro","contents":[{"role":"user","parts":[{"text":"hi"}]}]}"#,
                ),
                &stored,
                ProxyRoute::Gemini,
                Some("models/gemini-2.5-pro:generateContent"),
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(
            value.get("model").and_then(Value::as_str),
            Some("models/gemini-2.5-pro-preview")
        );
        assert_eq!(
            request.actual_model_source.as_deref(),
            Some("model_catalog")
        );
        assert_eq!(
            request.actual_model.as_deref(),
            Some("models/gemini-2.5-pro-preview")
        );
    }

    #[test]
    fn model_mapping_ordered_rules_support_route_app_provider_and_wildcards() {
        let adapter = adapter_for(AppKind::Codex, ProviderType::OpenRouter);
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::OpenRouter,
            json!({
                "env": {
                    "OPENAI_BASE_URL": "https://openrouter.ai/api/v1",
                    "OPENAI_API_KEY": "secret"
                },
                "modelMapping": {
                    "rules": [
                        {
                            "match": "gpt-5*",
                            "app": "claude",
                            "upstreamModel": "wrong-app"
                        },
                        {
                            "match": "gpt-5*",
                            "app": "codex",
                            "route": "responses",
                            "providerTypes": ["openrouter"],
                            "upstreamModel": "openai/gpt-5.5"
                        }
                    ],
                    "upstreamModel": "fallback-model"
                }
            }),
        );

        let request = adapter
            .transform_request_for_route(
                Bytes::from_static(br#"{"model":"gpt-5-mini","input":"hi","stream":false}"#),
                &stored,
                ProxyRoute::CodexResponses,
                None,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(
            value.get("model").and_then(Value::as_str),
            Some("openai/gpt-5.5")
        );
        assert_eq!(
            request.actual_model_source.as_deref(),
            Some("model_mapping_rule")
        );
    }

    #[test]
    fn model_mapping_route_mismatch_falls_back_to_legacy_upstream_model() {
        let adapter = adapter_for(AppKind::Codex, ProviderType::Codex);
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::Codex,
            json!({
                "env": {
                    "OPENAI_BASE_URL": "https://api.example",
                    "OPENAI_API_KEY": "secret"
                },
                "modelMapping": {
                    "rules": [{
                        "match": "gpt-5*",
                        "route": "chat_completions",
                        "upstreamModel": "chat-only-model"
                    }],
                    "upstreamModel": "legacy-default"
                }
            }),
        );

        let request = adapter
            .transform_request_for_route(
                Bytes::from_static(br#"{"model":"gpt-5-mini","input":"hi"}"#),
                &stored,
                ProxyRoute::CodexResponses,
                None,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(
            value.get("model").and_then(Value::as_str),
            Some("legacy-default")
        );
        assert_eq!(
            request.actual_model_source.as_deref(),
            Some("model_mapping")
        );
    }

    #[test]
    fn claude_api_key_contract_preserves_code_agent_fields() {
        let adapter = adapter_for(AppKind::Claude, ProviderType::Claude);
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::Claude,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://api.anthropic.com",
                    "ANTHROPIC_API_KEY": "secret"
                }
            }),
        );
        let request = adapter
            .transform_request(
                Bytes::from_static(
                    br#"{"model":"claude-sonnet-4","system":"s","metadata":{"user_id":"u1"},"thinking":{"type":"enabled","budget_tokens":1024},"messages":[{"role":"user","content":[{"type":"text","text":"hi"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA=="}}]},{"role":"assistant","content":[{"type":"tool_use","id":"tool-1","name":"lookup","input":{"q":"x"}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-1","content":"ok","cache_control":{"type":"ephemeral"}}]}],"stream":true}"#,
                ),
                &stored,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(request.model.as_deref(), Some("claude-sonnet-4"));
        assert!(request.stream_requested);
        assert_eq!(
            value.pointer("/thinking/type").and_then(Value::as_str),
            Some("enabled")
        );
        assert_eq!(
            value
                .pointer("/messages/0/content/1/source/media_type")
                .and_then(Value::as_str),
            Some("image/png")
        );
        assert_eq!(
            value
                .pointer("/messages/1/content/0/type")
                .and_then(Value::as_str),
            Some("tool_use")
        );
        assert_eq!(
            value
                .pointer("/messages/2/content/0/cache_control/type")
                .and_then(Value::as_str),
            Some("ephemeral")
        );
        assert_eq!(
            value.pointer("/metadata/user_id").and_then(Value::as_str),
            Some("u1")
        );
    }

    #[test]
    fn cache_injection_is_disabled_by_default_for_claude_body() {
        let adapter = adapter_for(AppKind::Claude, ProviderType::Claude);
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::Claude,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://api.anthropic.com",
                    "ANTHROPIC_API_KEY": "secret"
                }
            }),
        );

        let request = adapter
            .transform_request(
                Bytes::from_static(
                    br#"{"model":"claude-sonnet-4","tools":[{"name":"lookup"}],"system":[{"type":"text","text":"sys"}],"messages":[{"role":"assistant","content":[{"type":"text","text":"ok"}]}]}"#,
                ),
                &stored,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(count_cache_controls(&value), 0);
    }

    #[test]
    fn claude_native_cache_injection_adds_prompt_cache_breakpoints() {
        let adapter = adapter_for(AppKind::Claude, ProviderType::Claude);
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::Claude,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://api.anthropic.com",
                    "ANTHROPIC_API_KEY": "secret"
                },
                "cacheInjection": {"enabled": true, "ttl": "1h"}
            }),
        );

        let request = adapter
            .transform_request(
                Bytes::from_static(
                    br#"{"model":"claude-sonnet-4","tools":[{"name":"lookup"}],"system":"sys","messages":[{"role":"assistant","content":[{"type":"text","text":"ok"}]}]}"#,
                ),
                &stored,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(
            value
                .pointer("/tools/0/cache_control/ttl")
                .and_then(Value::as_str),
            Some("1h")
        );
        assert_eq!(
            value
                .pointer("/system/0/cache_control/ttl")
                .and_then(Value::as_str),
            Some("1h")
        );
        assert_eq!(
            value
                .pointer("/messages/0/content/0/cache_control/ttl")
                .and_then(Value::as_str),
            Some("1h")
        );
    }

    #[test]
    fn claude_to_codex_preserves_injected_cache_control_after_transform() {
        let adapter = adapter_for(AppKind::Claude, ProviderType::Codex);
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::Codex,
            json!({
                "env": {
                    "OPENAI_BASE_URL": "https://api.example",
                    "OPENAI_API_KEY": "secret"
                },
                "cacheInjection": {"enabled": true, "ttl": "1h"}
            }),
        );

        let request = adapter
            .transform_request(
                Bytes::from_static(
                    br#"{"model":"claude-sonnet-4","system":"sys","messages":[{"role":"assistant","content":[{"type":"text","text":"ok"}]}]}"#,
                ),
                &stored,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert!(value.get("input").and_then(Value::as_array).is_some());
        assert!(count_cache_controls(&value) >= 1);
    }

    #[test]
    fn codex_to_claude_injects_prompt_cache_after_transform() {
        let adapter = adapter_for(AppKind::Codex, ProviderType::Claude);
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::Claude,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://api.anthropic.com",
                    "ANTHROPIC_API_KEY": "secret"
                },
                "cache_injection": {"enabled": true, "ttl": "1h"}
            }),
        );

        let request = adapter
            .transform_request(
                Bytes::from_static(
                    br#"{"model":"gpt-5.5","messages":[{"role":"assistant","content":[{"type":"text","text":"ok"}]}]}"#,
                ),
                &stored,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();
        let assistant = value["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["role"] == "assistant")
            .unwrap();

        assert_eq!(
            assistant
                .pointer("/content/0/cache_control/ttl")
                .and_then(Value::as_str),
            Some("1h")
        );
    }

    #[test]
    fn thinking_optimizer_uses_mapped_anthropic_model_for_native_claude() {
        let adapter = adapter_for(AppKind::Claude, ProviderType::Claude);
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::Claude,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://api.anthropic.com",
                    "ANTHROPIC_API_KEY": "secret"
                },
                "modelMapping": {"upstreamModel": "anthropic.claude-sonnet-4-6-20250514-v1:0"},
                "thinkingPipeline": {"enabled": true}
            }),
        );

        let request = adapter
            .transform_request(
                Bytes::from_static(
                    br#"{"model":"alias","messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}]}"#,
                ),
                &stored,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(
            value.get("model").and_then(Value::as_str),
            Some("anthropic.claude-sonnet-4-6-20250514-v1:0")
        );
        assert_eq!(
            value.pointer("/thinking/type").and_then(Value::as_str),
            Some("adaptive")
        );
        assert_eq!(
            value
                .pointer("/output_config/effort")
                .and_then(Value::as_str),
            Some("max")
        );
    }

    #[test]
    fn codex_to_claude_reasoning_uses_mapped_model_without_optimizer() {
        let adapter = adapter_for(AppKind::Codex, ProviderType::Claude);
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::Claude,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://api.anthropic.com",
                    "ANTHROPIC_API_KEY": "secret"
                },
                "modelMapping": {"upstreamModel": "anthropic.claude-sonnet-4-6-20250514-v1:0"}
            }),
        );

        let request = adapter
            .transform_request(
                Bytes::from_static(
                    br#"{"model":"gpt-5.5","max_output_tokens":4096,"reasoning":{"effort":"high","summary":"auto"},"input":"ping"}"#,
                ),
                &stored,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(
            value.get("model").and_then(Value::as_str),
            Some("anthropic.claude-sonnet-4-6-20250514-v1:0")
        );
        assert_eq!(value["thinking"], json!({"type": "adaptive"}));
        assert_eq!(value["output_config"], json!({"effort": "high"}));
        assert!(value.pointer("/thinking/summary").is_none());
    }

    #[test]
    fn codex_to_claude_applies_thinking_optimizer_after_transform() {
        let expected_max_tokens = 8192;
        let adapter = adapter_for(AppKind::Codex, ProviderType::Claude);
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::Claude,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://api.anthropic.com",
                    "ANTHROPIC_API_KEY": "secret"
                },
                "modelMapping": {"upstreamModel": "anthropic.claude-sonnet-4-5-20250514-v1:0"},
                "thinking_pipeline": {"optimizer": true}
            }),
        );

        let request = adapter
            .transform_request(
                Bytes::from_static(
                    br#"{"model":"gpt-5.5","messages":[{"role":"user","content":"hi"}]}"#,
                ),
                &stored,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(
            value.pointer("/thinking/type").and_then(Value::as_str),
            Some("enabled")
        );
        assert_eq!(
            value
                .pointer("/thinking/budget_tokens")
                .and_then(Value::as_u64),
            Some(expected_max_tokens - 1)
        );
        assert_eq!(
            value.get("max_tokens").and_then(Value::as_u64),
            Some(expected_max_tokens)
        );
        assert!(value["anthropic_beta"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item.as_str() == Some("interleaved-thinking-2025-05-14")));
    }

    #[test]
    fn thinking_signature_rectifier_cleans_anthropic_request_when_explicitly_enabled() {
        let adapter = adapter_for(AppKind::Claude, ProviderType::Claude);
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::Claude,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://api.anthropic.com",
                    "ANTHROPIC_API_KEY": "secret"
                },
                "thinkingPipeline": {"signatureRectifier": true}
            }),
        );

        let request = adapter
            .transform_request(
                Bytes::from_static(
                    br#"{"model":"claude-sonnet-4","thinking":{"type":"enabled","budget_tokens":1024},"messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"t","signature":"sig1"},{"type":"text","text":"ok","signature":"sig2"},{"type":"tool_use","id":"toolu_1","name":"lookup","input":{},"signature":"sig3"}]}]}"#,
                ),
                &stored,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert!(value.get("thinking").is_none());
        assert_eq!(
            value
                .pointer("/messages/0/content/0/type")
                .and_then(Value::as_str),
            Some("text")
        );
        assert!(value.pointer("/messages/0/content/0/signature").is_none());
        assert_eq!(
            value
                .pointer("/messages/0/content/1/type")
                .and_then(Value::as_str),
            Some("tool_use")
        );
        assert!(value.pointer("/messages/0/content/1/signature").is_none());
    }

    #[test]
    fn openrouter_headers_include_optional_site_metadata() {
        let adapter = adapter_for(AppKind::Codex, ProviderType::OpenRouter);
        let stored = StoredProvider {
            app: AppKind::Codex,
            provider: crate::domain::providers::model::Provider {
                id: "p1".to_string(),
                name: "openrouter".to_string(),
                settings_config: json!({
                    "env": {
                        "OPENAI_API_KEY": "secret",
                        "OPENROUTER_SITE_URL": "https://cc-switch.example",
                        "OPENROUTER_APP_NAME": "cc-switch-server"
                    }
                }),
                category: None,
                meta: Some(crate::domain::providers::model::ProviderMeta {
                    custom_user_agent: Some("cc-switch-server-test".to_string()),
                    ..Default::default()
                }),
                extra: Default::default(),
            },
            provider_type: ProviderType::OpenRouter,
            provider_type_id: "openrouter".to_string(),
            resource: Default::default(),
        };

        let headers = adapter
            .build_headers(AppKind::Codex, &stored, &AccountStore::default())
            .unwrap();

        assert!(headers
            .iter()
            .any(|item| item == &("authorization", "Bearer secret".to_string())));
        assert!(headers
            .iter()
            .any(|item| item == &("http-referer", "https://cc-switch.example".to_string())));
        assert!(headers
            .iter()
            .any(|item| item == &("x-title", "cc-switch-server".to_string())));
        assert!(!headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("user-agent")));
    }

    #[test]
    fn openrouter_codex_contract_uses_openai_path_and_reasoning_fields() {
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::OpenRouter,
            json!({
                "env": {
                    "OPENAI_API_KEY": "secret",
                    "OPENAI_BASE_URL": "https://openrouter.ai/api"
                },
                "modelMapping": {
                    "upstreamModel": "openrouter/auto"
                }
            }),
        );

        assert_adapter_contract(AdapterContract {
            app: AppKind::Codex,
            provider_type: ProviderType::OpenRouter,
            route: ProxyRoute::CodexResponses,
            gemini_path: None,
            stored: stored.clone(),
            request_body:
                br#"{"model":"gpt-5.5","input":[{"role":"user","content":[{"type":"input_text","text":"ping"},{"type":"input_image","image_url":"data:image/png;base64,AA=="}]}],"reasoning":{"effort":"medium","summary":"auto"},"stream":true}"#,
            expected_endpoint: "https://openrouter.ai/api/v1/responses",
            expected_header: ("authorization", "Bearer secret"),
            expected_model: Some("openrouter/auto"),
            expected_stream: true,
        });

        let request = adapter_for(AppKind::Codex, ProviderType::OpenRouter)
            .transform_request(
                Bytes::from_static(
                    br#"{"model":"gpt-5.5","reasoning":{"effort":"medium","summary":"auto"},"stream":true}"#,
                ),
                &stored,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(
            value.pointer("/reasoning/effort").and_then(Value::as_str),
            Some("medium")
        );
        assert_eq!(
            value.pointer("/reasoning/summary").and_then(Value::as_str),
            Some("auto")
        );
    }

    #[test]
    fn request_governance_runs_after_model_mapping_for_text_only_models() {
        let adapter = adapter_for(AppKind::Codex, ProviderType::OpenRouter);
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::OpenRouter,
            json!({
                "env": {
                    "OPENAI_API_KEY": "secret",
                    "OPENAI_BASE_URL": "https://openrouter.ai/api"
                },
                "modelMapping": {"upstreamModel": "deepseek-v4-pro"},
                "modelCatalog": {
                    "models": [{"id": "deepseek-v4-pro", "supportsImage": false}]
                },
                "requestGovernance": {
                    "enabled": true,
                    "privateFieldWhitelist": ["_metadata"]
                }
            }),
        );

        let request = adapter
            .transform_request_for_route(
                Bytes::from_static(
                    br#"{"model":"alias","_debug":true,"_metadata":{"keep":true},"messages":[{"role":"user","content":[{"type":"text","text":"describe"},{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}}]}]}"#,
                ),
                &stored,
                ProxyRoute::CodexChatCompletions,
                None,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(
            value.get("model").and_then(Value::as_str),
            Some("deepseek-v4-pro")
        );
        assert!(value.get("_debug").is_none());
        assert_eq!(
            value.pointer("/_metadata/keep").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            value
                .pointer("/messages/0/content/1/type")
                .and_then(Value::as_str),
            Some("text")
        );
        assert_eq!(
            value
                .pointer("/messages/0/content/1/text")
                .and_then(Value::as_str),
            Some("[Unsupported Image]")
        );
    }

    #[test]
    fn request_governance_is_disabled_by_default() {
        let adapter = adapter_for(AppKind::Codex, ProviderType::OpenRouter);
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::OpenRouter,
            json!({
                "env": {
                    "OPENAI_API_KEY": "secret",
                    "OPENAI_BASE_URL": "https://openrouter.ai/api"
                },
                "modelMapping": {"upstreamModel": "deepseek-v4-pro"},
                "modelCatalog": {
                    "models": [{"id": "deepseek-v4-pro", "supportsImage": false}]
                }
            }),
        );

        let request = adapter
            .transform_request_for_route(
                Bytes::from_static(
                    br#"{"model":"alias","_debug":true,"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}}]}]}"#,
                ),
                &stored,
                ProxyRoute::CodexChatCompletions,
                None,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(
            value.get("model").and_then(Value::as_str),
            Some("deepseek-v4-pro")
        );
        assert_eq!(value.get("_debug").and_then(Value::as_bool), Some(true));
        assert_eq!(
            value
                .pointer("/messages/0/content/0/type")
                .and_then(Value::as_str),
            Some("image_url")
        );
    }

    #[test]
    fn claude_auth_uses_bearer_authorization_header() {
        let adapter = adapter_for(AppKind::Claude, ProviderType::ClaudeAuth);
        let stored = StoredProvider {
            app: AppKind::Claude,
            provider: crate::domain::providers::model::Provider {
                id: "p1".to_string(),
                name: "ClaudeAuth Relay".to_string(),
                settings_config: json!({
                    "env": {
                        "ANTHROPIC_BASE_URL": "https://relay.example",
                        "ANTHROPIC_AUTH_TOKEN": "secret"
                    },
                    "auth_mode": "bearer_only"
                }),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            provider_type: ProviderType::ClaudeAuth,
            provider_type_id: "claude_auth".to_string(),
            resource: Default::default(),
        };

        let headers = adapter
            .build_headers(AppKind::Claude, &stored, &AccountStore::default())
            .unwrap();

        assert!(headers
            .iter()
            .any(|item| item == &("authorization", "Bearer secret".to_string())));
        assert!(!headers.iter().any(|(name, _)| *name == "x-api-key"));
        assert!(headers
            .iter()
            .any(|item| item == &("anthropic-version", "2023-06-01".to_string())));
    }

    #[test]
    fn claude_auth_stream_contract_keeps_bearer_only() {
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::ClaudeAuth,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://relay.example",
                    "ANTHROPIC_AUTH_TOKEN": "secret"
                },
                "auth_mode": "bearer_only"
            }),
        );

        assert_adapter_contract(AdapterContract {
            app: AppKind::Claude,
            provider_type: ProviderType::ClaudeAuth,
            route: ProxyRoute::ClaudeMessages,
            gemini_path: None,
            stored,
            request_body: br#"{"model":"claude-sonnet-4","messages":[],"stream":true}"#,
            expected_endpoint: "https://relay.example/v1/messages",
            expected_header: ("authorization", "Bearer secret"),
            expected_model: Some("claude-sonnet-4"),
            expected_stream: true,
        });
    }

    #[test]
    fn codex_chat_to_anthropic_uses_direct_request_bridge() {
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::Claude,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://api.anthropic.example",
                    "ANTHROPIC_API_KEY": "secret"
                }
            }),
        );
        let request = adapter_for(AppKind::Codex, ProviderType::Claude)
            .transform_request_for_route(
                Bytes::from_static(
                    br#"{"model":"claude-sonnet-4-6","messages":[{"role":"system","content":"policy"},{"role":"user","content":"hello"}],"max_tokens":64}"#,
                ),
                &stored,
                ProxyRoute::CodexChatCompletions,
                None,
            )
            .unwrap();
        let body: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(body["system"], json!("policy"));
        assert_eq!(body["messages"][0]["role"], json!("user"));
        assert_eq!(
            body["messages"][0]["content"],
            json!([{"type": "text", "text": "hello"}])
        );
        assert_eq!(body["max_tokens"], 64);
        assert!(body.get("input").is_none());
    }

    #[test]
    fn codex_responses_contract_uses_openai_compatible_path() {
        let stored = StoredProvider {
            app: AppKind::Codex,
            provider: crate::domain::providers::model::Provider {
                id: "codex-1".to_string(),
                name: "Codex".to_string(),
                settings_config: json!({
                    "env": {
                        "OPENAI_BASE_URL": "https://api.example",
                        "OPENAI_API_KEY": "secret"
                    },
                    "modelMapping": {
                        "upstreamModel": "gpt-5-mini"
                    }
                }),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            provider_type: ProviderType::Codex,
            provider_type_id: "codex".to_string(),
            resource: Default::default(),
        };

        assert_adapter_contract(AdapterContract {
            app: AppKind::Codex,
            provider_type: ProviderType::Codex,
            route: ProxyRoute::CodexResponses,
            gemini_path: None,
            stored,
            request_body:
                br#"{"model":"gpt-5.5","input":"ping","stream":true,"reasoning":{"effort":"low"}}"#,
            expected_endpoint: "https://api.example/v1/responses",
            expected_header: ("authorization", "Bearer secret"),
            expected_model: Some("gpt-5-mini"),
            expected_stream: true,
        });
    }

    #[test]
    fn codex_chat_contract_preserves_cached_token_and_image_fields() {
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::Codex,
            json!({
                "env": {
                    "OPENAI_BASE_URL": "https://api.example",
                    "OPENAI_API_KEY": "secret"
                }
            }),
        );

        assert_adapter_contract(AdapterContract {
            app: AppKind::Codex,
            provider_type: ProviderType::Codex,
            route: ProxyRoute::CodexChatCompletions,
            gemini_path: None,
            stored: stored.clone(),
            request_body: br#"{"model":"gpt-5.5","messages":[{"role":"user","content":[{"type":"text","text":"describe"},{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}}]}],"stream":false}"#,
            expected_endpoint: "https://api.example/v1/chat/completions",
            expected_header: ("authorization", "Bearer secret"),
            expected_model: Some("gpt-5.5"),
            expected_stream: false,
        });

        let usage = adapter_for(AppKind::Codex, ProviderType::Codex).parse_usage(
            br#"{"usage":{"prompt_tokens":100,"completion_tokens":8,"prompt_tokens_details":{"cached_tokens":70}}}"#,
            &stored,
            ProxyRoute::CodexChatCompletions,
        );
        assert_eq!(usage.raw_input_tokens, Some(100));
        assert_eq!(usage.cache_read_tokens, Some(70));
    }

    #[test]
    fn codex_oauth_chat_completions_are_normalized_to_responses_upstream() {
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::CodexOAuth,
            json!({
                "env": {
                    "OPENAI_API_KEY": "oauth-token"
                }
            }),
        );

        let adapter = adapter_for(AppKind::Codex, ProviderType::CodexOAuth);
        let request = adapter
            .transform_request(
                Bytes::from_static(
                    br#"{"model":"gpt-5.5","messages":[{"role":"user","content":"ping"}],"max_completion_tokens":16,"reasoning_effort":"low","response_format":{"type":"json_object"},"stream":false}"#,
                ),
                &stored,
            )
            .unwrap();
        let endpoint = adapter
            .resolve_endpoint_for_request(ProxyRoute::CodexChatCompletions, None, &stored, &request)
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(
            capability_for(AppKind::Codex, ProviderType::CodexOAuth).support,
            AdapterSupport::Native
        );
        assert_eq!(endpoint, "https://chatgpt.com/backend-api/codex/responses");
        assert_eq!(
            value
                .pointer("/input/0/content/0/text")
                .and_then(Value::as_str),
            Some("ping")
        );
        assert_eq!(
            value.get("max_output_tokens").and_then(Value::as_i64),
            Some(16)
        );
        assert_eq!(
            value.pointer("/reasoning/effort").and_then(Value::as_str),
            Some("low")
        );
        assert_eq!(
            value.pointer("/text/format/type").and_then(Value::as_str),
            Some("json_object")
        );
        assert_eq!(request.model.as_deref(), Some("gpt-5.5"));
        assert!(!request.stream_requested);
    }

    #[test]
    fn grok_oauth_chat_completions_use_the_responses_upstream_contract() {
        let stored = stored_provider(AppKind::Codex, ProviderType::GrokOAuth, json!({"env": {}}));
        let adapter = adapter_for(AppKind::Codex, ProviderType::GrokOAuth);
        let request = adapter
            .transform_request(
                Bytes::from_static(
                    br#"{"model":"grok-4.6","messages":[{"role":"user","content":"ping"}],"stream":true}"#,
                ),
                &stored,
            )
            .unwrap();
        let endpoint = adapter
            .resolve_endpoint_for_request(ProxyRoute::CodexChatCompletions, None, &stored, &request)
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(endpoint, "https://api.x.ai/v1/responses");
        assert!(value.get("messages").is_none());
        assert_eq!(
            value
                .pointer("/input/0/content/0/text")
                .and_then(Value::as_str),
            Some("ping")
        );
        assert!(request.stream_requested);
    }

    #[test]
    fn claude_grok_oauth_uses_responses_function_tool_shape() {
        let stored = stored_provider(AppKind::Claude, ProviderType::GrokOAuth, json!({"env": {}}));
        let adapter = adapter_for(AppKind::Claude, ProviderType::GrokOAuth);
        let request = adapter
            .transform_request_for_route(
                Bytes::from_static(
                    br#"{"model":"grok-4.6","messages":[{"role":"user","content":"ping"}],"tools":[{"name":"lookup","description":"Lookup","input_schema":{"type":"object"}}]}"#,
                ),
                &stored,
                ProxyRoute::ClaudeMessages,
                None,
            )
            .unwrap();
        let endpoint = adapter
            .resolve_endpoint_for_request(ProxyRoute::ClaudeMessages, None, &stored, &request)
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(endpoint, "https://api.x.ai/v1/responses");
        assert_eq!(value.pointer("/tools/0/type"), Some(&json!("function")));
        assert_eq!(value.pointer("/tools/0/name"), Some(&json!("lookup")));
        assert!(value.pointer("/tools/0/function").is_none());
    }

    #[test]
    fn codex_oauth_responses_output_is_bridged_to_chat_completions_downstream() {
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::CodexOAuth,
            json!({"env": {"OPENAI_API_KEY": "oauth-token"}}),
        );
        let adapter = adapter_for(AppKind::Codex, ProviderType::CodexOAuth);
        let body = Bytes::from_static(
            br#"{"id":"resp_1","object":"response","status":"completed","model":"gpt-5.5","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}],"usage":{"input_tokens":10,"output_tokens":2,"total_tokens":12,"input_tokens_details":{"cached_tokens":4}}}"#,
        );

        let response = adapter
            .transform_response(body, &stored, ProxyRoute::CodexChatCompletions)
            .unwrap();
        let value: Value = serde_json::from_slice(&response).unwrap();

        assert_eq!(
            value.get("object").and_then(Value::as_str),
            Some("chat.completion")
        );
        assert_eq!(
            value
                .pointer("/choices/0/message/content")
                .and_then(Value::as_str),
            Some("hello")
        );
        assert_eq!(
            value
                .pointer("/usage/prompt_tokens_details/cached_tokens")
                .and_then(Value::as_i64),
            Some(4)
        );
    }

    #[test]
    fn codex_custom_provider_auth_json_builds_bearer_header() {
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::Codex,
            json!({
                "auth": { "OPENAI_API_KEY": "sk-custom-key" },
                "config": "base_url = \"https://relay.example/v1\"\n"
            }),
        );
        let accounts = AccountStore::default();
        let headers = adapter_for(AppKind::Codex, ProviderType::Codex)
            .build_headers(AppKind::Codex, &stored, &accounts)
            .unwrap();

        assert!(headers.contains(&("authorization", "Bearer sk-custom-key".to_string())));
    }

    #[test]
    fn codex_oauth_managed_account_headers_include_chatgpt_account_id() {
        let stored = codex_oauth_stored_provider_with_account_binding("acct-1");
        let mut accounts = AccountStore::default();
        accounts.accounts.push(codex_oauth_account(
            "acct-1",
            "access-token",
            json!({
                "verifiedOpenAiClaims": {
                    "chatgpt_account_id": "chatgpt-account-1"
                }
            }),
        ));

        let headers = adapter_for(AppKind::Codex, ProviderType::CodexOAuth)
            .build_headers(AppKind::Codex, &stored, &accounts)
            .unwrap();

        assert!(headers.contains(&("authorization", "Bearer access-token".to_string())));
        assert!(headers.contains(&("chatgpt-account-id", "chatgpt-account-1".to_string())));
    }

    #[test]
    fn managed_account_headers_require_explicit_binding_even_with_provider_secret() {
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::KiroOAuth,
            json!({"env": {"ANTHROPIC_API_KEY": "must-not-be-used"}}),
        );
        let mut fallback = codex_oauth_account("first-account", "first-token", json!({}));
        fallback.provider_type = ProviderType::KiroOAuth;
        let accounts = AccountStore {
            accounts: vec![fallback],
            ..Default::default()
        };
        let mut headers = Vec::new();

        let error =
            apply_auth_headers(&mut headers, AppKind::Claude, &stored, &accounts).unwrap_err();

        assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
        assert!(error
            .message
            .contains("managed account binding is required"));
    }

    #[test]
    fn managed_account_headers_use_only_the_bound_account() {
        let stored = codex_oauth_stored_provider_with_account_binding("bound-account");
        let accounts = AccountStore {
            accounts: vec![
                codex_oauth_account(
                    "first-account",
                    "first-token",
                    json!({
                        "verifiedOpenAiClaims": {
                            "chatgpt_account_id": "first-workspace"
                        }
                    }),
                ),
                codex_oauth_account(
                    "bound-account",
                    "bound-token",
                    json!({
                        "verifiedOpenAiClaims": {
                            "chatgpt_account_id": "bound-workspace"
                        }
                    }),
                ),
            ],
            ..Default::default()
        };

        let headers = adapter_for(AppKind::Codex, ProviderType::CodexOAuth)
            .build_headers(AppKind::Codex, &stored, &accounts)
            .unwrap();

        assert!(headers.contains(&("authorization", "Bearer bound-token".to_string())));
        assert!(headers.contains(&("chatgpt-account-id", "bound-workspace".to_string())));
        assert!(!headers
            .iter()
            .any(|(_, value)| value.contains("first-token") || value == "first-workspace"));
    }

    #[test]
    fn metadata_account_does_not_supply_static_provider_auth() {
        let stored = stored_provider(AppKind::Codex, ProviderType::DeepSeekApi, json!({}));
        let mut metadata = codex_oauth_account("metadata-account", "metadata-token", json!({}));
        metadata.provider_type = ProviderType::DeepSeekApi;
        metadata.api_key = Some("metadata-api-key".to_string());
        let accounts = AccountStore {
            accounts: vec![metadata],
            ..Default::default()
        };
        let mut headers = Vec::new();

        apply_auth_headers(&mut headers, AppKind::Codex, &stored, &accounts).unwrap();

        assert!(!headers
            .iter()
            .any(|(name, _)| matches!(*name, "authorization" | "x-api-key" | "x-goog-api-key")));
    }

    #[test]
    fn managed_account_headers_reject_stale_identity_generation() {
        let stored = codex_oauth_stored_provider_with_account_binding("acct-1");
        let mut account = codex_oauth_account(
            "acct-1",
            "new-identity-token",
            json!({
                "verifiedOpenAiClaims": {
                    "chatgpt_account_id": "chatgpt-account-2"
                }
            }),
        );
        account.auth_identity_generation = 2;
        let accounts = AccountStore {
            accounts: vec![account],
            ..Default::default()
        };

        let error = adapter_for(AppKind::Codex, ProviderType::CodexOAuth)
            .build_headers(AppKind::Codex, &stored, &accounts)
            .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
        assert!(error.message.contains("identity changed"));
    }

    #[test]
    fn codex_oauth_managed_account_rejects_missing_chatgpt_account_id() {
        let stored = codex_oauth_stored_provider_with_account_binding("acct-1");
        let mut accounts = AccountStore::default();
        accounts.accounts.push(codex_oauth_account(
            "acct-1",
            "access-token",
            json!({"email": "codex@example.test"}),
        ));

        let error = adapter_for(AppKind::Codex, ProviderType::CodexOAuth)
            .build_headers(AppKind::Codex, &stored, &accounts)
            .unwrap_err();

        assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
        assert!(error.message.contains("missing chatgpt_account_id"));
    }

    #[test]
    fn codex_upstream_join_deduplicates_openai_version_prefix() {
        assert_eq!(
            join_upstream_url("https://api.openai.com/v1", "/v1/responses"),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            join_upstream_url("https://api.openai.com", "/v1/responses"),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            join_upstream_url("https://chatgpt.com/backend-api/codex", "/v1/responses"),
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(
            join_upstream_url("https://chatgpt.com/backend-api/codex/v1", "/v1/responses"),
            "https://chatgpt.com/backend-api/codex/v1/responses"
        );
        assert_eq!(
            join_upstream_url("https://relay.example/openai", "/v1/responses"),
            "https://relay.example/openai/responses"
        );
    }

    #[test]
    fn ollama_cloud_codex_contract_uses_openai_compatible_auth() {
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::OllamaCloud,
            json!({
                "env": {
                    "OPENAI_BASE_URL": "https://ollama.com",
                    "OPENAI_API_KEY": "secret"
                }
            }),
        );

        assert_adapter_contract(AdapterContract {
            app: AppKind::Codex,
            provider_type: ProviderType::OllamaCloud,
            route: ProxyRoute::CodexChatCompletions,
            gemini_path: None,
            stored,
            request_body: br#"{"model":"gpt-oss:20b","messages":[],"stream":false}"#,
            expected_endpoint: "https://ollama.com/v1/chat/completions",
            expected_header: ("authorization", "Bearer secret"),
            expected_model: Some("gpt-oss:20b"),
            expected_stream: false,
        });
    }

    #[test]
    fn codex_responses_to_ollama_uses_chat_completions_upstream() {
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::OllamaCloud,
            json!({
                "env": {
                    "OPENAI_BASE_URL": "https://ollama.com",
                    "OPENAI_API_KEY": "secret"
                }
            }),
        );

        assert_adapter_contract(AdapterContract {
            app: AppKind::Codex,
            provider_type: ProviderType::OllamaCloud,
            route: ProxyRoute::CodexResponses,
            gemini_path: None,
            stored,
            request_body: br#"{"model":"gpt-oss:20b","input":"ping","stream":false}"#,
            expected_endpoint: "https://ollama.com/v1/chat/completions",
            expected_header: ("authorization", "Bearer secret"),
            expected_model: Some("gpt-oss:20b"),
            expected_stream: false,
        });
    }

    #[test]
    fn codex_responses_to_ollama_maps_xhigh_reasoning_effort_to_max() {
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::OllamaCloud,
            json!({
                "env": {
                    "OPENAI_BASE_URL": "https://ollama.com",
                    "OPENAI_API_KEY": "secret"
                }
            }),
        );

        let request = adapter_for(AppKind::Codex, ProviderType::OllamaCloud)
            .transform_request_for_route(
                Bytes::from_static(
                    br#"{"model":"gpt-5.5","input":"ping","reasoning":{"effort":"xhigh"},"stream":false}"#,
                ),
                &stored,
                ProxyRoute::CodexResponses,
                None,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(
            value.get("reasoning_effort").and_then(Value::as_str),
            Some("max")
        );
        assert!(value.get("reasoning").is_none());
    }

    #[test]
    fn codex_responses_to_ollama_passes_explicit_none_reasoning_effort() {
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::OllamaCloud,
            json!({
                "env": {
                    "OPENAI_BASE_URL": "https://ollama.com",
                    "OPENAI_API_KEY": "secret"
                }
            }),
        );

        let request = adapter_for(AppKind::Codex, ProviderType::OllamaCloud)
            .transform_request_for_route(
                Bytes::from_static(
                    br#"{"model":"gpt-5.5","input":"ping","reasoning":{"effort":"disabled"},"stream":false}"#,
                ),
                &stored,
                ProxyRoute::CodexResponses,
                None,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(
            value.get("reasoning_effort").and_then(Value::as_str),
            Some("none")
        );
        assert!(value.get("reasoning").is_none());
    }

    #[test]
    fn codex_responses_to_non_ollama_openai_chat_preserves_xhigh_reasoning_effort() {
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::Nvidia,
            json!({
                "env": {
                    "OPENAI_BASE_URL": "https://api.example",
                    "OPENAI_API_KEY": "secret"
                }
            }),
        );

        let request = adapter_for(AppKind::Codex, ProviderType::Nvidia)
            .transform_request_for_route(
                Bytes::from_static(
                    br#"{"model":"gpt-5.5","input":"ping","reasoning":{"effort":"xhigh"},"stream":false}"#,
                ),
                &stored,
                ProxyRoute::CodexResponses,
                None,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(
            value.get("reasoning_effort").and_then(Value::as_str),
            Some("xhigh")
        );
    }

    #[test]
    fn claude_ollama_preset_accepts_anthropic_named_key_for_chat_upstream() {
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::OllamaCloud,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://ollama.com",
                    "ANTHROPIC_AUTH_TOKEN": "secret"
                }
            }),
        );

        assert_adapter_contract(AdapterContract {
            app: AppKind::Claude,
            provider_type: ProviderType::OllamaCloud,
            route: ProxyRoute::ClaudeMessages,
            gemini_path: None,
            stored,
            request_body: br#"{"model":"kimi-k2.7-code","messages":[{"role":"user","content":"ping"}],"stream":false}"#,
            expected_endpoint: "https://ollama.com/v1/chat/completions",
            expected_header: ("authorization", "Bearer secret"),
            expected_model: Some("kimi-k2.7-code"),
            expected_stream: false,
        });
    }

    #[test]
    fn claude_cursor_apikey_agentservice_contract_uses_openai_chat_upstream_fixture() {
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::CursorApiKey,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://api.cursor.com",
                    "ANTHROPIC_AUTH_TOKEN": "secret"
                },
                "modelMapping": {
                    "upstreamModel": "composer-2.5"
                }
            }),
        );

        assert_eq!(
            capability_for(AppKind::Claude, ProviderType::CursorApiKey).support,
            AdapterSupport::Native
        );
        assert_adapter_contract(AdapterContract {
            app: AppKind::Claude,
            provider_type: ProviderType::CursorApiKey,
            route: ProxyRoute::ClaudeMessages,
            gemini_path: None,
            stored,
            request_body: br#"{"model":"claude-sonnet-4","messages":[{"role":"user","content":"ping"}],"stream":true}"#,
            expected_endpoint: "https://api.cursor.com/v1/chat/completions",
            expected_header: ("authorization", "Bearer secret"),
            expected_model: Some("composer-2.5"),
            expected_stream: true,
        });
    }

    #[test]
    fn claude_nvidia_contract_uses_openai_chat_transform() {
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::Nvidia,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://integrate.api.nvidia.com",
                    "ANTHROPIC_AUTH_TOKEN": "secret"
                },
                "modelMapping": {
                    "upstreamModel": "moonshotai/kimi-k2.5"
                }
            }),
        );

        assert_adapter_contract(AdapterContract {
            app: AppKind::Claude,
            provider_type: ProviderType::Nvidia,
            route: ProxyRoute::ClaudeMessages,
            gemini_path: None,
            stored,
            request_body: br#"{"model":"claude-sonnet-4","messages":[{"role":"user","content":"ping"}],"stream":false}"#,
            expected_endpoint: "https://integrate.api.nvidia.com/v1/chat/completions",
            expected_header: ("authorization", "Bearer secret"),
            expected_model: Some("moonshotai/kimi-k2.5"),
            expected_stream: false,
        });
    }

    #[test]
    fn codex_cursor_oauth_agentservice_contract_uses_chat_completions_fixture() {
        let mut stored = stored_provider(
            AppKind::Codex,
            ProviderType::CursorOAuth,
            json!({
                "env": {
                    "OPENAI_BASE_URL": "https://api2.cursor.sh",
                    "OPENAI_API_KEY": "secret"
                },
                "modelMapping": {
                    "upstreamModel": "composer-2.5"
                }
            }),
        );
        bind_managed_test_provider(&mut stored, "cursor-account");
        let accounts = AccountStore {
            accounts: vec![managed_test_account(
                ProviderType::CursorOAuth,
                "cursor-account",
                "secret",
                json!({}),
            )],
            ..Default::default()
        };

        assert_eq!(
            capability_for(AppKind::Codex, ProviderType::CursorOAuth).support,
            AdapterSupport::Native
        );
        assert_adapter_contract_with_accounts(
            AdapterContract {
                app: AppKind::Codex,
                provider_type: ProviderType::CursorOAuth,
                route: ProxyRoute::CodexResponses,
                gemini_path: None,
                stored,
                request_body: br#"{"model":"gpt-5.5","input":"ping","stream":false}"#,
                expected_endpoint: "https://api2.cursor.sh/v1/chat/completions",
                expected_header: ("authorization", "Bearer secret"),
                expected_model: Some("composer-2.5"),
                expected_stream: false,
            },
            &accounts,
        );
    }

    #[test]
    fn gemini_openrouter_contract_uses_openai_chat_with_gemini_key_alias() {
        let stored = stored_provider(
            AppKind::Gemini,
            ProviderType::OpenRouter,
            json!({
                "env": {
                    "GOOGLE_GEMINI_BASE_URL": "https://openrouter.ai/api",
                    "GEMINI_API_KEY": "secret"
                },
                "modelMapping": {
                    "upstreamModel": "google/gemini-3.5-flash"
                }
            }),
        );

        assert_eq!(
            capability_for(AppKind::Gemini, ProviderType::OpenRouter).support,
            AdapterSupport::Native
        );
        assert_adapter_contract(AdapterContract {
            app: AppKind::Gemini,
            provider_type: ProviderType::OpenRouter,
            route: ProxyRoute::Gemini,
            gemini_path: Some("models/gemini-3.5-flash:generateContent".to_string()),
            stored,
            request_body:
                br#"{"contents":[{"role":"user","parts":[{"text":"ping"}]}],"stream":false}"#,
            expected_endpoint: "https://openrouter.ai/api/v1/chat/completions",
            expected_header: ("authorization", "Bearer secret"),
            expected_model: Some("google/gemini-3.5-flash"),
            expected_stream: false,
        });
    }

    #[test]
    fn gemini_copilot_bridge_preserves_tools_usage_and_terminal_response() {
        let mut stored = stored_provider(
            AppKind::Gemini,
            ProviderType::GitHubCopilot,
            json!({
                "modelMapping": {"mode": "single", "upstreamModel": "gemini-3.5-flash"}
            }),
        );
        bind_managed_test_provider(&mut stored, "copilot-gemini-account");
        let adapter = adapter_for(AppKind::Gemini, ProviderType::GitHubCopilot);

        let request = adapter
            .transform_request_for_route(
                Bytes::from_static(
                    br#"{"contents":[{"role":"user","parts":[{"text":"look it up"}]}],"tools":[{"functionDeclarations":[{"name":"lookup","description":"Look up a value","parameters":{"type":"object","properties":{"q":{"type":"string"}},"required":["q"]}}]}]}"#,
                ),
                &stored,
                ProxyRoute::Gemini,
                Some("models/gemini-3.5-flash:streamGenerateContent"),
            )
            .unwrap();
        let endpoint = adapter
            .resolve_endpoint_for_request(
                ProxyRoute::Gemini,
                Some("models/gemini-3.5-flash:streamGenerateContent".to_string()),
                &stored,
                &request,
            )
            .unwrap();
        let request_body: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(endpoint, "https://api.githubcopilot.com/chat/completions");
        assert_eq!(request_body["model"], "gemini-3.5-flash");
        assert_eq!(request_body["tools"][0]["function"]["name"], "lookup");
        assert_eq!(
            request_body["tools"][0]["function"]["parameters"]["required"],
            json!(["q"])
        );
        assert_eq!(request_body["stream"], true);
        assert_eq!(request_body["stream_options"]["include_usage"], true);

        let response = adapter
            .transform_response(
                Bytes::from_static(
                    br#"{"id":"chatcmpl-copilot","model":"gemini-3.5-flash","choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_lookup","type":"function","function":{"name":"lookup","arguments":"{\"q\":\"rust\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":7,"completion_tokens":5,"total_tokens":12,"prompt_tokens_details":{"cached_tokens":2}}}"#,
                ),
                &stored,
                ProxyRoute::Gemini,
            )
            .unwrap();
        let response: Value = serde_json::from_slice(&response).unwrap();

        assert_eq!(
            response.pointer("/candidates/0/content/parts/0/functionCall/id"),
            Some(&json!("call_lookup"))
        );
        assert_eq!(
            response.pointer("/candidates/0/content/parts/0/functionCall/args/q"),
            Some(&json!("rust"))
        );
        assert_eq!(response["candidates"][0]["finishReason"], "STOP");
        assert_eq!(response["usageMetadata"]["promptTokenCount"], 7);
        assert_eq!(response["usageMetadata"]["cachedContentTokenCount"], 2);
        assert_eq!(response["usageMetadata"]["candidatesTokenCount"], 5);
    }

    #[test]
    fn gemini_stream_generate_content_forces_stream_on_openai_chat_upstream() {
        let adapter = adapter_for(AppKind::Gemini, ProviderType::OpenRouter);
        let stored = stored_provider(
            AppKind::Gemini,
            ProviderType::OpenRouter,
            json!({
                "env": {
                    "GOOGLE_GEMINI_BASE_URL": "https://openrouter.ai/api",
                    "GEMINI_API_KEY": "secret"
                },
                "modelMapping": {
                    "upstreamModel": "google/gemini-3.5-flash"
                }
            }),
        );

        let request = adapter
            .transform_request_for_route(
                Bytes::from_static(br#"{"contents":[{"role":"user","parts":[{"text":"ping"}]}]}"#),
                &stored,
                ProxyRoute::Gemini,
                Some("models/gemini-3.5-flash:streamGenerateContent"),
            )
            .unwrap();
        let endpoint = adapter
            .resolve_endpoint_for_request(
                ProxyRoute::Gemini,
                Some("models/gemini-3.5-flash:streamGenerateContent".to_string()),
                &stored,
                &request,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert!(request.stream_requested);
        assert_eq!(endpoint, "https://openrouter.ai/api/v1/chat/completions");
        assert_eq!(value.get("stream").and_then(Value::as_bool), Some(true));
        assert_eq!(
            value
                .pointer("/stream_options/include_usage")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            value
                .pointer("/messages/0/content/0/text")
                .and_then(Value::as_str),
            Some("ping")
        );
    }

    #[test]
    fn native_gemini_stream_generate_content_sets_stream_without_body_override() {
        let adapter = adapter_for(AppKind::Gemini, ProviderType::Gemini);
        let stored = stored_provider(
            AppKind::Gemini,
            ProviderType::Gemini,
            json!({
                "env": {
                    "GEMINI_BASE_URL": "https://generativelanguage.googleapis.com",
                    "GEMINI_API_KEY": "secret"
                }
            }),
        );

        let request = adapter
            .transform_request_for_route(
                Bytes::from_static(br#"{"contents":[{"role":"user","parts":[{"text":"ping"}]}]}"#),
                &stored,
                ProxyRoute::Gemini,
                Some("models/gemini-3.5-flash:streamGenerateContent"),
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert!(request.stream_requested);
        assert!(value.get("stream").is_none());
    }

    #[test]
    fn adapter_stream_transform_converts_openai_chat_to_gemini_stream() {
        let adapter = adapter_for(AppKind::Gemini, ProviderType::OpenRouter);
        let stored = stored_provider(
            AppKind::Gemini,
            ProviderType::OpenRouter,
            json!({"env": {"GOOGLE_GEMINI_BASE_URL": "https://openrouter.ai/api", "GEMINI_API_KEY": "secret"}}),
        );

        let response = adapter
            .transform_stream_event(
                Bytes::from_static(
                    br#"data: {"choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":null}]}

"#,
                ),
                &stored,
                ProxyRoute::Gemini,
            )
            .unwrap();
        let text = std::str::from_utf8(&response).unwrap();

        assert!(text.contains(r#""candidates""#));
        assert!(text.contains(r#""text":"hi""#));
    }

    #[test]
    fn bedrock_sigv4_plan_normalizes_region_scope_and_redaction_without_native_enablement() {
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::AwsBedrock,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://bedrock-runtime.${AWS_REGION}.amazonaws.com",
                    "AWS_REGION": "us-west-2",
                    "AWS_ACCESS_KEY_ID": "AKIA1234567890ABCD",
                    "AWS_SECRET_ACCESS_KEY": "secret",
                    "AWS_SESSION_TOKEN": "session"
                }
            }),
        );
        let request = AdapterRequest {
            body: Bytes::from_static(br#"{"messages":[]}"#),
            upstream_endpoint: None,
            upstream_headers: Vec::new(),
            model: Some("global.anthropic.claude-opus-4-8:0".to_string()),
            requested_model: Some("global.anthropic.claude-opus-4-8:0".to_string()),
            actual_model: None,
            actual_model_source: None,
            gemini_action: None,
            stream_requested: true,
            upstream_stream_requested: true,
            custom_tool_names: Default::default(),
            responses_tool_context: Default::default(),
            claude_tool_name_map: Default::default(),
        };
        let plan =
            bedrock_sigv4_request_plan(&stored, &request, "20260701", "20260701T000000Z").unwrap();

        assert_eq!(
            capability_for(AppKind::Claude, ProviderType::AwsBedrock).support,
            AdapterSupport::Planned
        );
        assert_eq!(plan.region, "us-west-2");
        assert_eq!(plan.service, "bedrock");
        assert_eq!(plan.host, "bedrock-runtime.us-west-2.amazonaws.com");
        assert_eq!(
            plan.credential_scope,
            "20260701/us-west-2/bedrock/aws4_request"
        );
        assert_eq!(
            plan.canonical_uri,
            "/model/global.anthropic.claude-opus-4-8%3A0/converse-stream"
        );
        assert_eq!(plan.amz_date, "20260701T000000Z");
        assert_eq!(
            plan.body
                .get("messages")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0)
        );
        assert!(plan
            .endpoint
            .ends_with("/model/global.anthropic.claude-opus-4-8%3A0/converse-stream"));
        assert_eq!(
            plan.signed_headers,
            "content-type;host;x-amz-content-sha256;x-amz-date;x-amz-security-token"
        );
        assert_eq!(plan.payload_hash.len(), 64);
        assert_eq!(plan.canonical_request_hash.len(), 64);
        assert!(plan.authorization_header.contains("Signature="));
        assert_eq!(plan.redacted_session_token.as_deref(), Some("<redacted>"));
        assert!(plan.redacted_authorization.contains("AKIA...ABCD"));
        assert!(!plan.redacted_authorization.contains("secret"));
        assert_eq!(plan.signing_status, "sigv4_signed");
    }

    #[test]
    fn bedrock_signed_request_parts_apply_sigv4_plan_without_enabling_forwarding() {
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::AwsBedrock,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://bedrock-runtime.${AWS_REGION}.amazonaws.com",
                    "AWS_REGION": "us-west-2",
                    "AWS_ACCESS_KEY_ID": "AKIA1234567890ABCD",
                    "AWS_SECRET_ACCESS_KEY": "secret",
                    "AWS_SESSION_TOKEN": "session"
                }
            }),
        );
        let request = AdapterRequest {
            body: Bytes::from_static(br#"{"messages":[{"role":"user","content":"ping"}]}"#),
            upstream_endpoint: None,
            upstream_headers: Vec::new(),
            model: Some("global.anthropic.claude-opus-4-8:0".to_string()),
            requested_model: Some("global.anthropic.claude-opus-4-8:0".to_string()),
            actual_model: None,
            actual_model_source: None,
            gemini_action: None,
            stream_requested: false,
            upstream_stream_requested: false,
            custom_tool_names: Default::default(),
            responses_tool_context: Default::default(),
            claude_tool_name_map: Default::default(),
        };

        let signed =
            bedrock_sigv4_signed_request_parts(&stored, &request, "20260701", "20260701T000000Z")
                .unwrap();
        let body = serde_json::from_slice::<Value>(&signed.body).unwrap();

        assert_eq!(
            capability_for(AppKind::Claude, ProviderType::AwsBedrock).support,
            AdapterSupport::Planned
        );
        assert!(signed.endpoint.ends_with("/converse"));
        assert_eq!(
            body.pointer("/messages/0/role").and_then(Value::as_str),
            Some("user")
        );
        assert_eq!(
            body.pointer("/messages/0/content/0/text")
                .and_then(Value::as_str),
            Some("ping")
        );
        assert_eq!(
            signed
                .headers
                .iter()
                .find(|(name, _)| *name == "host")
                .map(|(_, value)| value.as_str()),
            Some("bedrock-runtime.us-west-2.amazonaws.com")
        );
        assert_eq!(
            signed
                .headers
                .iter()
                .find(|(name, _)| *name == "x-amz-date")
                .map(|(_, value)| value.as_str()),
            Some("20260701T000000Z")
        );
        assert_eq!(
            signed
                .headers
                .iter()
                .find(|(name, _)| *name == "x-amz-security-token")
                .map(|(_, value)| value.as_str()),
            Some("session")
        );
        assert!(signed
            .headers
            .iter()
            .any(|(name, value)| *name == "authorization" && value.contains("Signature=")));
        assert_eq!(signed.plan.signing_status, "sigv4_signed");
        assert!(!signed.plan.redacted_authorization.contains("secret"));
        assert_eq!(
            signed.plan.redacted_session_token.as_deref(),
            Some("<redacted>")
        );
    }

    #[test]
    fn typed_bedrock_final_sigv4_headers_are_unique() {
        let mut stored = stored_provider(
            AppKind::Claude,
            ProviderType::AwsBedrock,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://bedrock-runtime.us-west-2.amazonaws.com",
                    "AWS_REGION": "us-west-2",
                    "AWS_ACCESS_KEY_ID": "AKIA1234567890ABCD",
                    "AWS_SECRET_ACCESS_KEY": "test-secret",
                    "AWS_SESSION_TOKEN": "test-session"
                }
            }),
        );
        stored.resource = crate::domain::providers::store::ProviderResourceMetadata {
            profile_id: Some(
                crate::domain::providers::registry::ProfileId::parse("claude.aws_bedrock_aksk")
                    .unwrap(),
            ),
            profile_schema_revision: Some(1),
            revision: 1,
            credential_generation: 1,
            ..Default::default()
        };
        let plan = crate::domain::providers::runtime::compile_runtime_plan(
            &stored,
            &AccountStore::default(),
        )
        .unwrap();
        assert_eq!(
            plan.configuration_state,
            crate::domain::providers::runtime::RuntimeConfigurationState::Ready
        );
        assert_eq!(plan.profile_id.as_str(), "claude.aws_bedrock_aksk");
        assert_eq!(plan.driver_id.as_str(), "aws.bedrock_sigv4");
        assert_eq!(
            capability_for(AppKind::Claude, ProviderType::AwsBedrock).support,
            AdapterSupport::Planned
        );

        let mut request = AdapterRequest {
            body: Bytes::from_static(
                br#"{"model":"global.anthropic.claude-opus-4-8","messages":[{"role":"user","content":"ping"}]}"#,
            ),
            upstream_endpoint: None,
            upstream_headers: Vec::new(),
            model: Some("global.anthropic.claude-opus-4-8".to_string()),
            requested_model: Some("global.anthropic.claude-opus-4-8".to_string()),
            actual_model: Some("global.anthropic.claude-opus-4-8".to_string()),
            actual_model_source: Some("typed_profile".to_string()),
            gemini_action: None,
            stream_requested: false,
            upstream_stream_requested: false,
            custom_tool_names: Default::default(),
            responses_tool_context: Default::default(),
            claude_tool_name_map: Default::default(),
        };
        let mut endpoint = plan.endpoint.clone();
        let mut target_headers = vec![
            ("Authorization".to_string(), "stale-signature".to_string()),
            ("authorization".to_string(), "older-signature".to_string()),
            (
                "Content-Type".to_string(),
                "application/client+json".to_string(),
            ),
            (
                "content-type".to_string(),
                "application/stale+json".to_string(),
            ),
            ("x-amz-date".to_string(), "20000101T000000Z".to_string()),
            (
                "x-amz-security-token".to_string(),
                "stale-session".to_string(),
            ),
        ];

        finalize_runtime_protocol_auth(
            &plan,
            &stored,
            &AccountStore::default(),
            &mut request,
            &mut endpoint,
            &mut target_headers,
        )
        .unwrap();

        for name in [
            "authorization",
            "content-type",
            "host",
            "x-amz-content-sha256",
            "x-amz-date",
            "x-amz-security-token",
        ] {
            assert_eq!(
                target_headers
                    .iter()
                    .filter(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
                    .count(),
                1,
                "header={name}"
            );
        }

        let mut client_headers = axum::http::HeaderMap::new();
        client_headers.insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/client+json"),
        );
        let headers = super::super::outbound_request::assemble_headers(
            &client_headers,
            &target_headers,
            "*/*",
            "application/json",
        )
        .unwrap();

        assert_eq!(headers.get_all("authorization").iter().count(), 1);
        assert!(headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("AWS4-HMAC-SHA256 Credential=")));
        assert_eq!(headers.get_all("content-type").iter().count(), 1);
        assert_eq!(
            headers
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        assert_eq!(headers.get_all("x-amz-date").iter().count(), 1);
        assert_eq!(headers.get_all("x-amz-security-token").iter().count(), 1);
        assert_eq!(
            headers
                .get("x-amz-security-token")
                .and_then(|value| value.to_str().ok()),
            Some("test-session")
        );
    }

    #[test]
    fn bedrock_signed_body_maps_anthropic_tools_results_and_inference_config() {
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::AwsBedrock,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://bedrock-runtime.us-west-2.amazonaws.com",
                    "AWS_REGION": "us-west-2",
                    "AWS_ACCESS_KEY_ID": "AKIA1234567890ABCD",
                    "AWS_SECRET_ACCESS_KEY": "secret",
                    "AWS_SESSION_TOKEN": "session-token"
                }
            }),
        );
        let request = AdapterRequest {
            body: Bytes::from_static(
                br#"{"model":"anthropic.claude-sonnet-4-6:0","max_tokens":512,"temperature":0.2,"tools":[{"name":"lookup","description":"Look up a value","input_schema":{"type":"object","properties":{"q":{"type":"string"}},"required":["q"]}}],"tool_choice":{"type":"tool","name":"lookup"},"messages":[{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"lookup","input":{"q":"ping"}},{"type":"tool_use","id":"toolu_2","name":"lookup","input":{"q":"fail"}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"pong"},{"type":"tool_result","tool_use_id":"toolu_2","content":"lookup failed","is_error":true}]}]}"#,
            ),
            upstream_endpoint: None,
            upstream_headers: Vec::new(),
            model: Some("anthropic.claude-sonnet-4-6:0".to_string()),
            requested_model: Some("anthropic.claude-sonnet-4-6:0".to_string()),
            actual_model: None,
            actual_model_source: None,
            gemini_action: None,
            stream_requested: false,
            upstream_stream_requested: false,
            custom_tool_names: Default::default(),
            responses_tool_context: Default::default(),
            claude_tool_name_map: Default::default(),
        };
        let signed =
            bedrock_sigv4_signed_request_parts(&stored, &request, "20260701", "20260701T000000Z")
                .unwrap();
        let body = serde_json::from_slice::<Value>(&signed.body).unwrap();

        assert_eq!(
            body.pointer("/messages/0/content/0/toolUse/name")
                .and_then(Value::as_str),
            Some("lookup")
        );
        assert_eq!(
            body.pointer("/messages/1/content/0/toolResult/toolUseId")
                .and_then(Value::as_str),
            Some("toolu_1")
        );
        assert_eq!(
            body.pointer("/messages/1/content/0/toolResult/status")
                .and_then(Value::as_str),
            Some("success")
        );
        assert_eq!(
            body.pointer("/messages/1/content/1/toolResult/status")
                .and_then(Value::as_str),
            Some("error")
        );
        assert_eq!(
            body.pointer("/toolConfig/tools/0/toolSpec/name")
                .and_then(Value::as_str),
            Some("lookup")
        );
        assert_eq!(
            body.pointer("/toolConfig/tools/0/toolSpec/description")
                .and_then(Value::as_str),
            Some("Look up a value")
        );
        assert_eq!(
            body.pointer("/toolConfig/tools/0/toolSpec/inputSchema/json/required/0")
                .and_then(Value::as_str),
            Some("q")
        );
        assert_eq!(
            body.pointer("/toolConfig/toolChoice/tool/name")
                .and_then(Value::as_str),
            Some("lookup")
        );
        assert_eq!(
            body.pointer("/inferenceConfig/maxTokens")
                .and_then(Value::as_u64),
            Some(512)
        );
        assert_eq!(signed.plan.payload_hash, sha256_hex(&signed.body));
        assert_eq!(
            signed
                .headers
                .iter()
                .find(|(name, _)| *name == "x-amz-security-token")
                .map(|(_, value)| value.as_str()),
            Some("session-token")
        );
    }

    #[test]
    fn bedrock_converse_body_maps_all_anthropic_tool_choices() {
        for (tool_choice, expected) in [
            (json!({ "type": "auto" }), json!({ "auto": {} })),
            (json!({ "type": "any" }), json!({ "any": {} })),
            (
                json!({ "type": "tool", "name": "lookup" }),
                json!({ "tool": { "name": "lookup" } }),
            ),
        ] {
            let input = json!({
                "messages": [{ "role": "user", "content": "ping" }],
                "tools": [{
                    "name": "lookup",
                    "description": "Look up a value",
                    "input_schema": { "type": "object" }
                }],
                "tool_choice": tool_choice,
            });

            let body = bedrock_converse_body_from_anthropic(&input).unwrap();

            assert_eq!(body.pointer("/toolConfig/toolChoice"), Some(&expected));
        }
    }

    #[test]
    fn bedrock_claude_route_builds_signed_forward_contract_but_remains_planned() {
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::AwsBedrock,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://bedrock-runtime.${AWS_REGION}.amazonaws.com",
                    "AWS_REGION": "us-west-2",
                    "AWS_ACCESS_KEY_ID": "AKIA1234567890ABCD",
                    "AWS_SECRET_ACCESS_KEY": "secret"
                }
            }),
        );
        let adapter = adapter_for(AppKind::Claude, ProviderType::AwsBedrock);

        let request = adapter
            .transform_request_for_route(
                Bytes::from_static(
                    br#"{"model":"anthropic.claude-sonnet-4-6:0","max_tokens":1,"messages":[{"role":"user","content":"ping"}]}"#,
                ),
                &stored,
                ProxyRoute::ClaudeMessages,
                None,
            )
            .unwrap();
        let body = serde_json::from_slice::<Value>(&request.body).unwrap();
        let headers = adapter
            .build_headers(AppKind::Claude, &stored, &AccountStore::default())
            .unwrap();

        assert_eq!(
            capability_for(AppKind::Claude, ProviderType::AwsBedrock).support,
            AdapterSupport::Planned
        );
        assert!(headers.is_empty());
        assert!(request
            .upstream_endpoint
            .as_deref()
            .is_some_and(|endpoint| endpoint.ends_with("/converse")));
        assert!(request
            .upstream_headers
            .iter()
            .any(|(name, value)| *name == "authorization" && value.contains("Signature=")));
        assert_eq!(
            body.pointer("/messages/0/content/0/text")
                .and_then(Value::as_str),
            Some("ping")
        );
    }

    #[test]
    fn bedrock_converse_plan_maps_anthropic_messages_without_enabling_adapter() {
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::AwsBedrock,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://bedrock-runtime.us-east-1.amazonaws.com",
                    "AWS_ACCESS_KEY_ID": "AKIA1234567890ABCD",
                    "AWS_SECRET_ACCESS_KEY": "secret"
                }
            }),
        );
        let request = AdapterRequest {
            body: Bytes::from_static(
                br#"{"system":"system prompt","max_tokens":64,"temperature":0.2,"top_p":0.9,"stop_sequences":["END"],"messages":[{"role":"user","content":[{"type":"text","text":"ping"}]}]}"#,
            ),
            upstream_endpoint: None,
            upstream_headers: Vec::new(),
            model: Some("anthropic.claude-sonnet-4-6:0".to_string()),
            requested_model: Some("anthropic.claude-sonnet-4-6:0".to_string()),
            actual_model: None,
            actual_model_source: None,
            gemini_action: None,
            stream_requested: false,
            upstream_stream_requested: false,
            custom_tool_names: Default::default(),
            responses_tool_context: Default::default(),
            claude_tool_name_map: Default::default(),
        };

        let plan =
            bedrock_sigv4_request_plan(&stored, &request, "20260701", "20260701T000000Z").unwrap();

        assert_eq!(
            capability_for(AppKind::Claude, ProviderType::AwsBedrock).support,
            AdapterSupport::Planned
        );
        assert!(plan.canonical_uri.ends_with("/converse"));
        assert_eq!(
            plan.body.pointer("/system/0/text").and_then(Value::as_str),
            Some("system prompt")
        );
        assert_eq!(
            plan.body
                .pointer("/messages/0/content/0/text")
                .and_then(Value::as_str),
            Some("ping")
        );
        assert_eq!(
            plan.body
                .pointer("/inferenceConfig/maxTokens")
                .and_then(Value::as_u64),
            Some(64)
        );
        assert_eq!(
            plan.body
                .pointer("/inferenceConfig/stopSequences/0")
                .and_then(Value::as_str),
            Some("END")
        );
    }

    #[test]
    fn bedrock_region_can_be_extracted_from_runtime_host() {
        assert_eq!(
            bedrock_region_from_base_url("https://bedrock-runtime.eu-central-1.amazonaws.com")
                .as_deref(),
            Some("eu-central-1")
        );
        assert_eq!(
            bedrock_region_from_base_url(
                "https://bedrock-runtime.us-east-1.vpce-012345.amazonaws.com"
            )
            .as_deref(),
            Some("us-east-1")
        );
    }

    #[test]
    fn gemini_nvidia_and_deepseek_contracts_use_openai_chat() {
        let nvidia = stored_provider(
            AppKind::Gemini,
            ProviderType::Nvidia,
            json!({
                "env": {
                    "GOOGLE_GEMINI_BASE_URL": "https://integrate.api.nvidia.com/v1",
                    "OPENAI_API_KEY": "secret"
                },
                "modelMapping": {
                    "upstreamModel": "moonshotai/kimi-k2.5"
                }
            }),
        );
        assert_adapter_contract(AdapterContract {
            app: AppKind::Gemini,
            provider_type: ProviderType::Nvidia,
            route: ProxyRoute::Gemini,
            gemini_path: Some("models/gemini-3.5-flash:generateContent".to_string()),
            stored: nvidia,
            request_body:
                br#"{"contents":[{"role":"user","parts":[{"text":"ping"}]}],"stream":false}"#,
            expected_endpoint: "https://integrate.api.nvidia.com/v1/chat/completions",
            expected_header: ("authorization", "Bearer secret"),
            expected_model: Some("moonshotai/kimi-k2.5"),
            expected_stream: false,
        });

        let deepseek = stored_provider(
            AppKind::Gemini,
            ProviderType::DeepSeekApi,
            json!({
                "env": {
                    "GOOGLE_GEMINI_BASE_URL": "https://api.deepseek.com",
                    "OPENAI_API_KEY": "secret"
                },
                "modelMapping": {
                    "upstreamModel": "deepseek-v4-flash"
                }
            }),
        );
        assert_adapter_contract(AdapterContract {
            app: AppKind::Gemini,
            provider_type: ProviderType::DeepSeekApi,
            route: ProxyRoute::Gemini,
            gemini_path: Some("models/gemini-3.5-flash:generateContent".to_string()),
            stored: deepseek,
            request_body:
                br#"{"contents":[{"role":"user","parts":[{"text":"ping"}]}],"stream":false}"#,
            expected_endpoint: "https://api.deepseek.com/v1/chat/completions",
            expected_header: ("authorization", "Bearer secret"),
            expected_model: Some("deepseek-v4-flash"),
            expected_stream: false,
        });
    }

    #[test]
    fn claude_codex_oauth_uses_anthropic_base_url_for_responses_upstream() {
        let mut stored = stored_provider(
            AppKind::Claude,
            ProviderType::CodexOAuth,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://chatgpt.com/backend-api/codex",
                    "ANTHROPIC_AUTH_TOKEN": "secret"
                }
            }),
        );
        bind_managed_test_provider(&mut stored, "codex-account");
        let accounts = AccountStore {
            accounts: vec![managed_test_account(
                ProviderType::CodexOAuth,
                "codex-account",
                "secret",
                json!({
                    "verifiedOpenAiClaims": {
                        "chatgpt_account_id": "codex-workspace"
                    }
                }),
            )],
            ..Default::default()
        };

        assert_adapter_contract_with_accounts(
            AdapterContract {
                app: AppKind::Claude,
                provider_type: ProviderType::CodexOAuth,
                route: ProxyRoute::ClaudeMessages,
                gemini_path: None,
                stored,
                request_body: br#"{"model":"gpt-5.5","messages":[{"role":"user","content":"ping"}],"stream":false}"#,
                expected_endpoint: "https://chatgpt.com/backend-api/codex/responses",
                expected_header: ("authorization", "Bearer secret"),
                expected_model: Some("gpt-5.5"),
                expected_stream: false,
            },
            &accounts,
        );
    }

    #[test]
    fn gemini_contract_preserves_schema_safety_and_tools() {
        let adapter = adapter_for(AppKind::Gemini, ProviderType::Gemini);
        let stored = stored_provider(
            AppKind::Gemini,
            ProviderType::Gemini,
            json!({
                "env": {
                    "GEMINI_BASE_URL": "https://generativelanguage.googleapis.com",
                    "GEMINI_API_KEY": "secret"
                }
            }),
        );
        let request = adapter
            .transform_request(
                Bytes::from_static(
                    br#"{"contents":[{"role":"user","parts":[{"text":"ping"}]}],"generationConfig":{"responseMimeType":"application/json","responseSchema":{"type":"object","properties":{"answer":{"type":"string"}}}},"safetySettings":[{"category":"HARM_CATEGORY_DANGEROUS_CONTENT","threshold":"BLOCK_NONE"}],"tools":[{"functionDeclarations":[{"name":"lookup","parameters":{"type":"object"}}]}],"stream":true}"#,
                ),
                &stored,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert!(request.stream_requested);
        assert_eq!(
            value
                .pointer("/generationConfig/responseSchema/type")
                .and_then(Value::as_str),
            Some("object")
        );
        assert_eq!(
            value
                .pointer("/safetySettings/0/threshold")
                .and_then(Value::as_str),
            Some("BLOCK_NONE")
        );
        assert_eq!(
            value
                .pointer("/tools/0/functionDeclarations/0/name")
                .and_then(Value::as_str),
            Some("lookup")
        );
    }

    #[test]
    fn adapter_transforms_openai_response_back_to_claude_client_shape() {
        let adapter = adapter_for(AppKind::Claude, ProviderType::Codex);
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::Codex,
            json!({"env": {"OPENAI_API_KEY": "secret"}}),
        );

        let response = adapter
            .transform_response(
                Bytes::from_static(
                    br#"{"id":"resp_1","status":"completed","model":"gpt-5.5","output":[{"type":"message","content":[{"type":"output_text","text":"hello"}]}],"usage":{"input_tokens":10,"output_tokens":2,"input_tokens_details":{"cached_tokens":4}}}"#,
                ),
                &stored,
                ProxyRoute::ClaudeMessages,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&response).unwrap();

        assert_eq!(value.get("type").and_then(Value::as_str), Some("message"));
        assert_eq!(
            value.pointer("/content/0/text").and_then(Value::as_str),
            Some("hello")
        );
        assert_eq!(
            value
                .pointer("/usage/cache_read_input_tokens")
                .and_then(Value::as_i64),
            Some(4)
        );
    }

    #[test]
    fn adapter_uses_codex_route_to_select_responses_or_chat_output_shape() {
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::Claude,
            json!({"env": {"ANTHROPIC_API_KEY": "secret"}}),
        );
        let adapter = adapter_for(AppKind::Codex, ProviderType::Claude);
        let anthropic = Bytes::from_static(
            br#"{"id":"msg_1","type":"message","role":"assistant","model":"claude-sonnet-4","content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":2}}"#,
        );

        let responses = adapter
            .transform_response(anthropic.clone(), &stored, ProxyRoute::CodexResponses)
            .unwrap();
        let responses_value: Value = serde_json::from_slice(&responses).unwrap();
        assert_eq!(
            responses_value.get("object").and_then(Value::as_str),
            Some("response")
        );
        assert_eq!(
            responses_value
                .pointer("/output/0/content/0/type")
                .and_then(Value::as_str),
            Some("output_text")
        );

        let chat = adapter
            .transform_response(anthropic, &stored, ProxyRoute::CodexChatCompletions)
            .unwrap();
        let chat_value: Value = serde_json::from_slice(&chat).unwrap();
        assert_eq!(
            chat_value.get("object").and_then(Value::as_str),
            Some("chat.completion")
        );
        assert_eq!(
            chat_value
                .pointer("/choices/0/message/content")
                .and_then(Value::as_str),
            Some("hello")
        );
    }

    #[test]
    fn adapter_transforms_openai_response_back_to_gemini_client_shape() {
        let adapter = adapter_for(AppKind::Gemini, ProviderType::Codex);
        let stored = stored_provider(
            AppKind::Gemini,
            ProviderType::Codex,
            json!({"env": {"OPENAI_API_KEY": "secret"}}),
        );

        let response = adapter
            .transform_response(
                Bytes::from_static(
                    br#"{"id":"resp_1","status":"completed","model":"gpt-5.5","output":[{"type":"message","content":[{"type":"output_text","text":"hello"}]}],"usage":{"input_tokens":10,"output_tokens":2}}"#,
                ),
                &stored,
                ProxyRoute::Gemini,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&response).unwrap();

        assert_eq!(
            value
                .pointer("/candidates/0/content/parts/0/text")
                .and_then(Value::as_str),
            Some("hello")
        );
        assert_eq!(
            value
                .pointer("/usageMetadata/promptTokenCount")
                .and_then(Value::as_i64),
            Some(10)
        );
    }

    #[test]
    fn adapter_stream_transform_converts_common_text_delta_events() {
        let adapter = adapter_for(AppKind::Claude, ProviderType::Codex);
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::Codex,
            json!({"env": {"OPENAI_API_KEY": "secret"}}),
        );

        let response = adapter
            .transform_stream_event(
                Bytes::from_static(
                    br#"data: {"type":"response.output_text.delta","delta":"hi"}

"#,
                ),
                &stored,
                ProxyRoute::ClaudeMessages,
            )
            .unwrap();
        let text = std::str::from_utf8(&response).unwrap();

        assert!(text.contains("event: content_block_delta"));
        assert!(text.contains(r#""text":"hi""#));
    }

    #[test]
    fn adapter_stream_transform_handles_crlf_multi_frame_sse_chunks() {
        let adapter = adapter_for(AppKind::Gemini, ProviderType::OpenRouter);
        let stored = stored_provider(
            AppKind::Gemini,
            ProviderType::OpenRouter,
            json!({"env": {"GOOGLE_GEMINI_BASE_URL": "https://openrouter.ai/api", "GEMINI_API_KEY": "secret"}}),
        );

        let response = adapter
            .transform_stream_event(
                Bytes::from_static(
                    b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"he\"},\"finish_reason\":null}]}\r\n\r\ndata: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"llo\"},\"finish_reason\":\"stop\"}]}\r\n\r\n",
                ),
                &stored,
                ProxyRoute::Gemini,
            )
            .unwrap();
        let text = std::str::from_utf8(&response).unwrap();

        assert!(text.contains(r#""text":"he""#));
        assert!(text.contains(r#""text":"llo""#));
        assert!(text.contains(r#""finishReason":"STOP""#));
    }

    #[test]
    fn adapter_cross_protocol_response_transform_rejects_error_shapes() {
        let adapter = adapter_for(AppKind::Claude, ProviderType::Codex);
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::Codex,
            json!({"env": {"OPENAI_API_KEY": "secret"}}),
        );
        let body = Bytes::from_static(br#"{"error":{"message":"bad key"}}"#);

        let error = adapter
            .transform_response(body.clone(), &stored, ProxyRoute::ClaudeMessages)
            .unwrap_err();

        assert_eq!(error.status, axum::http::StatusCode::BAD_GATEWAY);
        assert!(error.message.contains("error payload"));
    }

    #[test]
    fn adapter_cross_protocol_response_transform_rejects_invalid_json() {
        let adapter = adapter_for(AppKind::Claude, ProviderType::Codex);
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::Codex,
            json!({"env": {"OPENAI_API_KEY": "secret"}}),
        );

        let error = adapter
            .transform_response(
                Bytes::from_static(b"not-json"),
                &stored,
                ProxyRoute::ClaudeMessages,
            )
            .unwrap_err();

        assert_eq!(error.status, axum::http::StatusCode::BAD_GATEWAY);
        assert!(error.message.contains("invalid JSON"));
    }

    #[test]
    fn adapter_same_protocol_response_preserves_error_shapes() {
        let adapter = adapter_for(AppKind::Codex, ProviderType::Codex);
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::Codex,
            json!({"env": {"OPENAI_API_KEY": "secret"}}),
        );
        let body = Bytes::from_static(br#"{"error":{"message":"bad key"}}"#);

        let response = adapter
            .transform_response(body.clone(), &stored, ProxyRoute::CodexResponses)
            .unwrap();

        assert_eq!(response, body);
    }
}
