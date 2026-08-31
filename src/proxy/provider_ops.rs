use std::sync::Arc;

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;
use zeroize::Zeroize;

use crate::domain::accounts::managers::{
    account_credential_ownership, manager_for, AccountCredentialOwnership, AccountManager,
    CredentialKind,
};
use crate::domain::accounts::store::{effective_codex_workspace_id, Account, AccountStore};
use crate::domain::providers::coding_plan::CodingPlanRoute;
use crate::domain::providers::credentials::reveal_provider_credential;
use crate::domain::providers::model::{AppKind, CodexImageToolStripPolicy, ProviderType};
use crate::domain::providers::registry::{
    provider_registry, AuthScheme, OperationSupport, UpstreamProtocol,
};
use crate::domain::providers::runtime::{
    ProviderRuntimePlan, RuntimeAuthRef, RuntimeConfigurationState, RuntimeModelPolicy,
};
use crate::domain::providers::store::{ProviderStore, StoredProvider};

#[cfg(test)]
use super::account_headers::account_header_override_blocked;
use super::account_headers::apply_account_header_overrides;
use super::adapters::{self, AdapterRequest, ProviderAdapter};
use super::claude_oauth::ClaudeBodyRetryStage;
use super::router::ProxyRoute;
use super::{codex_provider_api_key, setting, ProxyError};

#[derive(Clone)]
pub(crate) struct ProviderExecution {
    pub stored: StoredProvider,
    pub plan: Arc<ProviderRuntimePlan>,
}

pub(crate) struct PreparedProviderRequest {
    pub adapter_request: AdapterRequest,
    pub endpoint: String,
    pub headers: Vec<(String, String)>,
    pub session_id: Option<String>,
}

impl std::fmt::Debug for ProviderExecution {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderExecution")
            .field("app", &self.stored.app)
            .field("provider_id", &self.stored.provider.id)
            .field("provider_revision", &self.stored.resource.revision)
            .field("driver_id", &self.plan.driver_id)
            .finish()
    }
}

impl Drop for ProviderExecution {
    fn drop(&mut self) {
        crate::domain::providers::credentials::zeroize_materialized_provider(
            &mut self.stored.provider,
        );
    }
}

impl ProviderExecution {
    pub fn from_store(store: &ProviderStore, stored: StoredProvider) -> Result<Self, ProxyError> {
        let execution = Self::from_store_for_operation(store, stored)?;
        execution.ensure_ready()?;
        Ok(execution)
    }

    pub fn from_store_for_operation(
        store: &ProviderStore,
        stored: StoredProvider,
    ) -> Result<Self, ProxyError> {
        let plan = store
            .runtime_plan(stored.app, &stored.provider.id)
            .ok_or_else(|| ProxyError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!(
                    "Provider {} has no committed runtime plan",
                    stored.provider.id
                ),
            })?;
        if plan.provider_revision != stored.resource.revision
            || plan.provider_key.app != stored.app
            || plan.provider_key.provider_id != stored.provider.id
        {
            return Err(ProxyError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!(
                    "Provider {} record and runtime plan generations do not match",
                    stored.provider.id
                ),
            });
        }
        let stored = store
            .materialize_provider_record(&stored)
            .map_err(|error| ProxyError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!(
                    "Provider {} credentials could not be materialized: {error}",
                    stored.provider.id
                ),
            })?;
        Ok(Self { stored, plan })
    }

    pub fn ensure_ready(&self) -> Result<(), ProxyError> {
        if self.plan.configuration_state == RuntimeConfigurationState::NeedsAttention {
            return Err(ProxyError::bad_request(format!(
                "Provider {} runtime configuration needs attention: {}",
                self.stored.provider.id,
                self.plan.warnings.join("; ")
            )));
        }
        if matches!(
            self.stored.provider_type,
            ProviderType::GrokOAuth | ProviderType::KimiCode
        ) && !matches!(
            &self.plan.auth_ref,
            RuntimeAuthRef::ManagedAccount {
                account_id,
                expected_provider_type,
                ..
            } if !account_id.trim().is_empty()
                && *expected_provider_type == self.stored.provider_type
        ) {
            return Err(ProxyError::bad_request(format!(
                "Provider {} must explicitly bind a {} managed account",
                self.stored.provider.id,
                self.stored.provider_type.as_str()
            )));
        }
        if matches!(
            self.plan.auth_ref,
            RuntimeAuthRef::Legacy {
                account_id: None,
                ..
            }
        ) && account_credential_ownership(self.stored.provider_type)
            == AccountCredentialOwnership::ManagedAccount
        {
            return Err(ProxyError::bad_request(format!(
                "Provider {} must explicitly bind a {} managed account",
                self.stored.provider.id,
                self.stored.provider_type.as_str()
            )));
        }
        Ok(())
    }

    pub fn driver_is(&self, driver_id: &str) -> bool {
        self.plan.driver_id.as_str() == driver_id
    }

    pub fn is_legacy(&self) -> bool {
        self.plan.configuration_state == RuntimeConfigurationState::LegacyCompat
            || self.plan.upstream_protocol == UpstreamProtocol::Legacy
    }

    pub fn runtime_stored_view(&self) -> StoredProvider {
        let mut stored = self.stored.clone();
        if self.is_legacy() {
            return stored;
        }
        let api_format = match self.plan.upstream_protocol {
            UpstreamProtocol::AnthropicMessages => Some("anthropic"),
            UpstreamProtocol::OpenAiChat => Some("openai_chat"),
            UpstreamProtocol::OpenAiResponses => Some("openai_responses"),
            UpstreamProtocol::GeminiNative => Some("gemini_native"),
            UpstreamProtocol::Special => match self.plan.driver_id.as_str() {
                "special.cursor" | "special.copilot" | "special.qoder_cosy" => Some("openai_chat"),
                "special.grok_web_session" | "special.perplexity_web_session" => {
                    Some(match self.plan.provider_key.app {
                        AppKind::Claude => "anthropic",
                        AppKind::Codex | AppKind::Gemini => "openai_chat",
                    })
                }
                "special.antigravity" | "special.agy" => Some("gemini_native"),
                "oauth.kimi_code" => Some(match self.plan.provider_key.app {
                    AppKind::Claude => "anthropic",
                    AppKind::Codex | AppKind::Gemini => "openai_chat",
                }),
                _ => None,
            },
            UpstreamProtocol::Bedrock | UpstreamProtocol::Custom | UpstreamProtocol::Legacy => None,
        };
        if let Some(api_format) = api_format {
            stored
                .provider
                .meta
                .get_or_insert_with(Default::default)
                .api_format = Some(api_format.to_string());
        }
        stored
    }

    pub fn managed_account_id(&self) -> Option<&str> {
        match &self.plan.auth_ref {
            RuntimeAuthRef::ManagedAccount { account_id, .. } => Some(account_id),
            RuntimeAuthRef::Legacy { account_id, .. }
                if account_credential_ownership(self.stored.provider_type)
                    == AccountCredentialOwnership::ManagedAccount =>
            {
                account_id.as_deref()
            }
            _ => None,
        }
    }

    pub fn managed_account_target(&self) -> Option<(ProviderType, &str)> {
        match &self.plan.auth_ref {
            RuntimeAuthRef::ManagedAccount {
                account_id,
                expected_provider_type,
                ..
            } => Some((*expected_provider_type, account_id.as_str())),
            RuntimeAuthRef::Legacy {
                account_id: Some(account_id),
                ..
            } if account_credential_ownership(self.stored.provider_type)
                == AccountCredentialOwnership::ManagedAccount =>
            {
                Some((self.stored.provider_type, account_id.as_str()))
            }
            _ => None,
        }
    }

    pub fn managed_account_identity_target(&self) -> Option<(ProviderType, &str, u64)> {
        match &self.plan.auth_ref {
            RuntimeAuthRef::ManagedAccount {
                account_id,
                expected_provider_type,
                auth_identity_generation,
            } => Some((
                *expected_provider_type,
                account_id.as_str(),
                *auth_identity_generation,
            )),
            _ => None,
        }
    }

    pub fn prepare_claude_request(
        &self,
        body: Bytes,
        route: ProxyRoute,
        client_headers: &HeaderMap,
        accounts: &AccountStore,
        retry_stage: Option<ClaudeBodyRetryStage>,
    ) -> Result<PreparedProviderRequest, ProxyError> {
        if !self.driver_is("oauth.claude_messages") {
            return Err(ProxyError::bad_request(format!(
                "driver {} does not implement the Claude OAuth request contract",
                self.plan.driver_id
            )));
        }
        let stored = self.runtime_stored_view();
        let adapter = adapters::adapter_for(stored.app, stored.provider_type);
        let request = adapter.transform_request_for_route(body, &stored, route, None)?;
        self.finalize_claude_request(request, route, client_headers, accounts, retry_stage)
    }

    pub fn finalize_claude_request(
        &self,
        mut request: AdapterRequest,
        route: ProxyRoute,
        client_headers: &HeaderMap,
        accounts: &AccountStore,
        retry_stage: Option<ClaudeBodyRetryStage>,
    ) -> Result<PreparedProviderRequest, ProxyError> {
        if !self.driver_is("oauth.claude_messages") {
            return Err(ProxyError::bad_request(format!(
                "driver {} does not implement the Claude OAuth request contract",
                self.plan.driver_id
            )));
        }
        let stored = self.runtime_stored_view();
        let adapter = adapters::adapter_for(stored.app, stored.provider_type);
        self.enforce_model_policy(&mut request)?;
        let context_1m_requested = normalize_native_claude_context_1m_model(&mut request)?;
        self.finalize_request(&mut request)?;
        let mut endpoint = self.resolve_endpoint(route, None, &request)?;
        let mut headers = adapter
            .build_headers(stored.app, &stored, accounts)?
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect::<Vec<_>>();
        for (name, value) in &request.upstream_headers {
            replace_header(&mut headers, name, value);
        }
        let identity_seed = self.managed_account_id().unwrap_or(&stored.provider.id);
        let contract = if route == ProxyRoute::ClaudeCountTokens {
            request.stream_requested = false;
            request.upstream_stream_requested = false;
            super::claude_oauth::apply_count_tokens_forward_contract(
                &mut endpoint,
                &mut request.body,
                client_headers,
                identity_seed,
                context_1m_requested,
            )?
        } else {
            super::claude_oauth::apply_forward_contract(
                &mut endpoint,
                &mut request.body,
                client_headers,
                identity_seed,
                context_1m_requested,
                retry_stage,
            )?
        };
        request.claude_tool_name_map = contract.tool_name_map;
        for (name, value) in contract.headers {
            replace_header(&mut headers, name, &value);
        }
        let materialized_auth = self.materialize_auth(accounts)?;
        self.apply_auth(&mut headers, &mut endpoint, &materialized_auth)?;
        apply_account_header_overrides(&mut headers, &stored, accounts)?;
        self.finalize_outbound_identity(&mut headers)?;
        Ok(PreparedProviderRequest {
            adapter_request: request,
            endpoint,
            headers,
            session_id: contract.session_id,
        })
    }

    pub fn ensure_operation_supported(
        &self,
        operation: ProviderOperation,
    ) -> Result<(), ProxyError> {
        let driver = provider_registry()
            .drivers
            .iter()
            .find(|driver| driver.driver_id == self.plan.driver_id)
            .ok_or_else(|| ProxyError::bad_request("Provider runtime driver is not registered"))?;
        let support = match operation {
            ProviderOperation::Forward => driver.operations.forward,
            ProviderOperation::Test => driver.operations.test,
            ProviderOperation::Discovery => driver.operations.discovery,
            ProviderOperation::Connectivity => driver.operations.connectivity,
        };
        if support == OperationSupport::Unsupported {
            return Err(ProxyError {
                status: StatusCode::NOT_IMPLEMENTED,
                message: format!(
                    "driver {} does not support {}",
                    self.plan.driver_id,
                    operation.as_str()
                ),
            });
        }
        Ok(())
    }

    pub fn materialize_auth(&self, accounts: &AccountStore) -> Result<AuthApplication, ProxyError> {
        let mut materialized = match &self.plan.auth_ref {
            RuntimeAuthRef::ManagedAccount {
                account_id,
                expected_provider_type,
                auth_identity_generation,
            } => {
                let account = exact_account(accounts, account_id).ok_or_else(|| {
                    ProxyError::bad_request(format!("bound account {account_id} does not exist"))
                })?;
                if account.provider_type != *expected_provider_type {
                    return Err(ProxyError::bad_request(format!(
                        "bound account {account_id} has provider type {}, expected {}",
                        account.provider_type.as_str(),
                        expected_provider_type.as_str()
                    )));
                }
                if account.auth_identity_generation != *auth_identity_generation {
                    return Err(ProxyError::conflict(format!(
                        "bound account {account_id} identity changed; rebind the Provider"
                    )));
                }
                if account.needs_relogin {
                    return Err(ProxyError {
                        status: StatusCode::UNAUTHORIZED,
                        message: format!("bound account {account_id} requires login"),
                    });
                }
                if managed_auth_is_protocol_owned(self) {
                    AuthApplication::ProtocolOwned
                } else {
                    let credential = manager_for(*expected_provider_type)
                        .get_valid_token(
                            accounts,
                            *expected_provider_type,
                            Some(account_id),
                            now_ms_i64(),
                        )
                        .map_err(|error| {
                            ProxyError::bad_request(format!(
                                "bound account {account_id} credential is unavailable: {error}"
                            ))
                        })?;
                    managed_auth(self, account, credential)
                }
            }
            RuntimeAuthRef::StaticCredential {
                auth_scheme,
                slots,
                credential_generation,
            } => {
                self.ensure_credential_generation(*credential_generation)?;
                let secret = self.provider_secret_from_slots(slots, false)?;
                static_auth(self, *auth_scheme, secret)?
            }
            RuntimeAuthRef::CustomCredential {
                auth_scheme,
                slots,
                credential_generation,
            } => {
                self.ensure_credential_generation(*credential_generation)?;
                if *auth_scheme == AuthScheme::None {
                    AuthApplication::NoAuth(MaterializedAuth::default())
                } else {
                    let secret = self.provider_secret_from_slots(slots, true)?;
                    static_auth(self, *auth_scheme, secret)?
                }
            }
            RuntimeAuthRef::AwsCredential {
                credential_generation,
                ..
            } => {
                self.ensure_credential_generation(*credential_generation)?;
                AuthApplication::ProtocolOwned
            }
            RuntimeAuthRef::Legacy { .. } => AuthApplication::LegacyPreserve,
            RuntimeAuthRef::Missing => {
                return Err(ProxyError::bad_request(format!(
                    "Provider {} credential binding is incomplete",
                    self.stored.provider.id
                )))
            }
        };
        if let Some(auth) = materialized.values_mut() {
            self.append_extra_headers(auth)?;
        }
        Ok(materialized)
    }

    fn provider_secret_from_slots(
        &self,
        slots: &[String],
        custom: bool,
    ) -> Result<String, ProxyError> {
        let mut candidates = Vec::new();
        for slot in slots {
            if !slot.starts_with('/')
                || self
                    .plan
                    .extra_headers
                    .iter()
                    .any(|header| header.credential_slot == *slot)
                || (!custom && !static_credential_slot_allowed(self, slot))
            {
                continue;
            }
            if let Ok(value) = reveal_provider_credential(&self.stored.provider, slot) {
                let value = value.trim();
                if !value.is_empty() {
                    candidates.push((slot.as_str(), value.to_string()));
                }
            }
        }

        candidates.sort_by_key(|(slot, _)| credential_slot_priority(slot));
        let mut unique = Vec::new();
        for candidate in candidates {
            if !unique.iter().any(|(_, value)| value == &candidate.1) {
                unique.push(candidate);
            }
        }
        match unique.as_slice() {
            [] => Err(ProxyError::bad_request(if custom {
                "custom Provider credential is not configured"
            } else {
                "Provider credential is not configured"
            })),
            [(_, value)] => Ok(value.clone()),
            _ => Err(ProxyError::bad_request(format!(
                "Provider has conflicting credentials in runtime slots: {}",
                unique
                    .iter()
                    .map(|(slot, _)| *slot)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    fn append_extra_headers(&self, auth: &mut MaterializedAuth) -> Result<(), ProxyError> {
        if self.plan.extra_headers.is_empty() {
            return Ok(());
        }
        let provider = serde_json::to_value(&self.stored.provider)
            .map_err(|error| ProxyError::bad_request(format!("encode Provider: {error}")))?;
        for header in &self.plan.extra_headers {
            let value = provider
                .pointer(&header.credential_slot)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ProxyError::bad_request(format!(
                        "custom extra header {} credential is not configured",
                        header.name
                    ))
                })?;
            HeaderValue::from_str(value).map_err(|_| {
                ProxyError::bad_request(format!(
                    "custom extra header {} has an invalid value",
                    header.name
                ))
            })?;
            replace_header(&mut auth.headers, &header.name, value);
        }
        Ok(())
    }

    fn ensure_credential_generation(&self, expected: u64) -> Result<(), ProxyError> {
        if self.stored.resource.credential_generation != expected {
            return Err(ProxyError::conflict(format!(
                "Provider {} credential generation changed",
                self.stored.provider.id
            )));
        }
        Ok(())
    }

    pub fn apply_auth(
        &self,
        headers: &mut Vec<(String, String)>,
        url: &mut String,
        auth: &AuthApplication,
    ) -> Result<(), ProxyError> {
        let auth = match auth {
            AuthApplication::Inject(auth) | AuthApplication::NoAuth(auth) => auth,
            AuthApplication::ProtocolOwned | AuthApplication::LegacyPreserve => return Ok(()),
        };
        headers.retain(|(name, _)| !canonical_auth_header(name));
        for (name, value) in &auth.headers {
            HeaderValue::from_str(value).map_err(|_| {
                ProxyError::bad_request(format!(
                    "materialized credential is not a valid value for header {name}"
                ))
            })?;
            replace_header(headers, name, value);
        }
        if !auth.query.is_empty() {
            let mut parsed = Url::parse(url).map_err(|error| {
                ProxyError::bad_request(format!("invalid upstream URL: {error}"))
            })?;
            let mut authoritative: Vec<(String, String)> = Vec::with_capacity(auth.query.len());
            for (name, value) in &auth.query {
                if let Some((_, current)) = authoritative
                    .iter_mut()
                    .find(|(current, _)| current == name)
                {
                    *current = value.clone();
                } else {
                    authoritative.push((name.clone(), value.clone()));
                }
            }
            let retained = parsed
                .query_pairs()
                .filter(|(name, _)| {
                    !authoritative
                        .iter()
                        .any(|(auth_name, _)| name.as_ref() == auth_name)
                })
                .map(|(name, value)| (name.into_owned(), value.into_owned()))
                .collect::<Vec<_>>();
            {
                let mut query = parsed.query_pairs_mut();
                query.clear();
                query.extend_pairs(
                    retained
                        .iter()
                        .map(|(name, value)| (name.as_str(), value.as_str())),
                );
                for (name, value) in &authoritative {
                    query.append_pair(name, value);
                }
            }
            *url = parsed.to_string();
        }
        Ok(())
    }

    pub fn finalize_outbound_identity(
        &self,
        headers: &mut Vec<(String, String)>,
    ) -> Result<(), ProxyError> {
        super::outbound_identity::finalize_headers(&self.plan, headers)
    }

    pub fn enforce_model_policy(&self, request: &mut AdapterRequest) -> Result<(), ProxyError> {
        let requested = request
            .requested_model
            .clone()
            .or_else(|| request_model(&request.body))
            .or_else(|| request.model.clone());
        match &self.plan.model_policy {
            RuntimeModelPolicy::Passthrough => {
                if request.requested_model.is_none() {
                    request.requested_model = requested.clone();
                }
                if request.actual_model.is_none() {
                    request.actual_model = requested.clone();
                }
                request.model = request.actual_model.clone().or(requested);
            }
            RuntimeModelPolicy::Single { upstream_model } => {
                let upstream_model = upstream_model.trim();
                if upstream_model.is_empty() {
                    return Err(ProxyError::bad_request(
                        "single-model Provider has no upstream model",
                    ));
                }
                if runtime_model_is_body_field(self) {
                    request.body = replace_request_model(&request.body, upstream_model)?;
                }
                request.requested_model = requested;
                request.model = Some(upstream_model.to_string());
                request.actual_model = Some(upstream_model.to_string());
                request.actual_model_source = Some("runtime_plan_single_model".to_string());
            }
        }
        Ok(())
    }

    pub fn finalize_request(&self, request: &mut AdapterRequest) -> Result<(), ProxyError> {
        adapters::finalize_runtime_request(&self.plan, &self.stored, request)
    }

    pub fn finalize_protocol_auth(
        &self,
        accounts: &AccountStore,
        request: &mut AdapterRequest,
        endpoint: &mut String,
        headers: &mut Vec<(String, String)>,
    ) -> Result<(), ProxyError> {
        adapters::finalize_runtime_protocol_auth(
            &self.plan,
            &self.stored,
            accounts,
            request,
            endpoint,
            headers,
        )
    }

    pub fn guard_coding_plan_request(
        &self,
        route: ProxyRoute,
        request: &AdapterRequest,
        endpoint: &str,
    ) -> Result<(), ProxyError> {
        let Some(contract) = self.plan.coding_plan.as_ref() else {
            return Ok(());
        };
        let contract_route = coding_plan_route(route)?;
        let model = request
            .actual_model
            .clone()
            .or_else(|| request.model.clone())
            .or_else(|| request_model(&request.body))
            .ok_or_else(|| ProxyError::bad_request("coding-plan request has no model"))?;
        if !contract.allows_model(&model) {
            return Err(ProxyError::bad_request(format!(
                "model {model} is not in the coding-plan Registry catalog"
            )));
        }
        contract
            .guard_final_endpoint(contract_route, endpoint)
            .map_err(|error| ProxyError::bad_request(error.to_string()))
    }

    pub fn apply_openai_codex_final_request_contract(
        &self,
        route: ProxyRoute,
        request: &mut AdapterRequest,
        prompt_cache_key: Option<&str>,
        responses_lite: bool,
        intent: &super::codex_request_policy::CodexRequestIntent,
    ) -> Result<super::codex_request_policy::CodexRequestPolicyMetadata, ProxyError> {
        if !self.driver_is("oauth.openai_codex")
            || self.plan.upstream_protocol != UpstreamProtocol::OpenAiResponses
        {
            return Err(ProxyError::bad_request(format!(
                "driver {} does not implement the OpenAI Codex OAuth Responses contract",
                self.plan.driver_id
            )));
        }

        let explicit_compact = route == ProxyRoute::CodexResponsesCompact;
        let body_signal =
            super::forwarder::codex_responses_body_has_compaction_trigger(&request.body);
        if explicit_compact {
            request.body =
                super::forwarder::normalize_codex_oauth_compact_body_bytes(&request.body)?;
            request.stream_requested = false;
            request.upstream_stream_requested = true;
        } else {
            let image_tool_strip_policy = self
                .stored
                .provider
                .meta
                .as_ref()
                .and_then(|meta| meta.codex_image_tool_strip_policy)
                .unwrap_or(CodexImageToolStripPolicy::Never);
            request.body = super::forwarder::normalize_codex_oauth_responses_body_bytes(
                &request.body,
                prompt_cache_key,
                image_tool_strip_policy,
            )?;
            if body_signal {
                request.body =
                    super::forwarder::normalize_codex_oauth_compaction_signal_body_bytes(
                        &request.body,
                        false,
                    )?;
            }
            if responses_lite {
                request.body = super::forwarder::normalize_codex_responses_lite_body_bytes(
                    &request.body,
                    true,
                    true,
                )?;
            }
            request.upstream_stream_requested = true;
        }

        let final_model = request_model(&request.body)
            .or_else(|| request.actual_model.clone())
            .or_else(|| request.model.clone());
        let fast_mode_enabled = self
            .plan
            .driver_options
            .get("codexFastMode")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let (body, metadata) = super::codex_request_policy::apply_to_bytes(
            &request.body,
            &self.runtime_stored_view(),
            final_model.as_deref(),
            fast_mode_enabled,
            intent,
        )?;
        request.body = super::codex_http::finalize_body(&body)?;
        Ok(metadata)
    }

    pub fn gate_openai_codex_responses_lite(
        &self,
        request: &AdapterRequest,
        requested: bool,
    ) -> bool {
        if !requested || !self.driver_is("oauth.openai_codex") {
            crate::metrics::record_codex_responses_lite_decision("not_requested");
            return false;
        }
        let final_model = request_model(&request.body)
            .or_else(|| request.actual_model.clone())
            .or_else(|| request.model.clone());
        match final_model.as_deref().map(|model| {
            super::codex_models::responses_lite_support(&self.runtime_stored_view(), model)
        }) {
            Some(super::codex_models::CapabilitySupport::Supported) => {
                crate::metrics::record_codex_responses_lite_decision("requested_supported");
                true
            }
            Some(super::codex_models::CapabilitySupport::Unsupported) => {
                crate::metrics::record_codex_responses_lite_decision(
                    "requested_unsupported_stripped",
                );
                false
            }
            Some(super::codex_models::CapabilitySupport::Unknown) | None => {
                crate::metrics::record_codex_responses_lite_decision("requested_unknown_preserved");
                true
            }
        }
    }

    pub fn resolve_endpoint(
        &self,
        route: ProxyRoute,
        gemini_path: Option<String>,
        request: &AdapterRequest,
    ) -> Result<String, ProxyError> {
        if let Some(contract) = self.plan.coding_plan.as_ref() {
            return contract
                .endpoint_for_route(coding_plan_route(route)?)
                .map_err(|error| ProxyError::bad_request(error.to_string()));
        }
        if self.plan.endpoint.trim().is_empty() {
            return Err(ProxyError::bad_request(
                "Provider endpoint is not configured",
            ));
        }
        adapters::resolve_runtime_endpoint_for_request(
            &self.plan,
            route,
            gemini_path,
            &self.stored,
            request,
        )
    }

    pub fn apply_test_forward_contract(
        &self,
        route: ProxyRoute,
        request: &mut AdapterRequest,
        endpoint: &mut String,
        headers: &mut Vec<(String, String)>,
    ) -> Result<(), ProxyError> {
        if self.driver_is("oauth.claude_messages") {
            let contract = if route == ProxyRoute::ClaudeCountTokens {
                super::claude_oauth::apply_count_tokens_forward_contract(
                    endpoint,
                    &mut request.body,
                    &HeaderMap::new(),
                    self.managed_account_id()
                        .unwrap_or(&self.stored.provider.id),
                    false,
                )?
            } else {
                super::claude_oauth::apply_forward_contract(
                    endpoint,
                    &mut request.body,
                    &HeaderMap::new(),
                    self.managed_account_id()
                        .unwrap_or(&self.stored.provider.id),
                    false,
                    None,
                )?
            };
            for (name, value) in contract.headers {
                replace_header(headers, name, &value);
            }
        }
        if self.driver_is("oauth.grok_responses") {
            let contract = super::grok::apply_forward_contract(
                &mut request.body,
                &HeaderMap::new(),
                route,
                None,
                None,
                None,
                matches!(self.plan.auth_ref, RuntimeAuthRef::ManagedAccount { .. }),
            )?;
            request.model = Some(contract.actual_model.clone());
            request.actual_model = Some(contract.actual_model.clone());
            request.actual_model_source = Some("grok_model_normalization".to_string());
            for (name, value) in contract.headers {
                replace_header(headers, name, &value);
            }
            *endpoint = super::grok::chat_upstream_url(
                endpoint,
                matches!(self.plan.auth_ref, RuntimeAuthRef::ManagedAccount { .. }),
            );
        }
        if self.driver_is("oauth.openai_codex") {
            let intent = super::codex_request_policy::extract_intent_from_bytes(&request.body);
            self.apply_openai_codex_final_request_contract(route, request, None, false, &intent)?;
            if route == ProxyRoute::CodexResponsesCompact {
                *endpoint = super::codex_compaction::responses_url(endpoint);
            }
        }
        Ok(())
    }

    pub fn discovery_url(&self) -> Result<String, ProxyError> {
        self.ensure_operation_supported(ProviderOperation::Discovery)?;
        adapters::runtime_model_list_url(&self.plan).ok_or_else(|| ProxyError {
            status: StatusCode::NOT_IMPLEMENTED,
            message: format!(
                "driver {} does not define a model discovery endpoint",
                self.plan.driver_id
            ),
        })
    }

    pub fn request_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.plan.transport_policy.timeout_ms.max(1))
    }

    pub fn stream_first_byte_timeout(&self) -> Option<std::time::Duration> {
        self.plan
            .transport_policy
            .stream_first_byte_timeout_ms
            .map(std::time::Duration::from_millis)
    }

    pub fn stream_idle_timeout(&self) -> Option<std::time::Duration> {
        self.plan
            .transport_policy
            .stream_idle_timeout_ms
            .map(std::time::Duration::from_millis)
    }
}

fn coding_plan_route(route: ProxyRoute) -> Result<CodingPlanRoute, ProxyError> {
    match route {
        ProxyRoute::ClaudeMessages => Ok(CodingPlanRoute::ClaudeMessages),
        ProxyRoute::ClaudeCountTokens => Ok(CodingPlanRoute::ClaudeCountTokens),
        ProxyRoute::CodexChatCompletions => Ok(CodingPlanRoute::CodexChatCompletions),
        ProxyRoute::CodexResponses => Ok(CodingPlanRoute::CodexResponses),
        ProxyRoute::CodexResponsesCompact => Err(ProxyError {
            status: StatusCode::NOT_IMPLEMENTED,
            message: "coding-plan contract does not support Responses compact".to_string(),
        }),
        ProxyRoute::Gemini => Err(ProxyError::bad_request(
            "coding-plan contract does not support the Gemini route",
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProviderOperation {
    Forward,
    Test,
    Discovery,
    Connectivity,
}

impl ProviderOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Forward => "forward",
            Self::Test => "test",
            Self::Discovery => "discovery",
            Self::Connectivity => "connectivity",
        }
    }
}

#[derive(Clone)]
pub(crate) enum AuthApplication {
    Inject(MaterializedAuth),
    ProtocolOwned,
    NoAuth(MaterializedAuth),
    LegacyPreserve,
}

impl AuthApplication {
    fn values_mut(&mut self) -> Option<&mut MaterializedAuth> {
        match self {
            Self::Inject(auth) | Self::NoAuth(auth) => Some(auth),
            Self::ProtocolOwned | Self::LegacyPreserve => None,
        }
    }

    pub(crate) fn injected_values(&self) -> Option<&MaterializedAuth> {
        match self {
            Self::Inject(auth) => Some(auth),
            Self::ProtocolOwned | Self::NoAuth(_) | Self::LegacyPreserve => None,
        }
    }
}

impl std::fmt::Debug for AuthApplication {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Inject(auth) => formatter.debug_tuple("Inject").field(auth).finish(),
            Self::ProtocolOwned => formatter.write_str("ProtocolOwned"),
            Self::NoAuth(auth) => formatter.debug_tuple("NoAuth").field(auth).finish(),
            Self::LegacyPreserve => formatter.write_str("LegacyPreserve"),
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct MaterializedAuth {
    pub headers: Vec<(String, String)>,
    pub query: Vec<(String, String)>,
}

impl std::fmt::Debug for MaterializedAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MaterializedAuth")
            .field("header_count", &self.headers.len())
            .field("query_count", &self.query.len())
            .finish()
    }
}

impl Drop for MaterializedAuth {
    fn drop(&mut self) {
        for (name, value) in &mut self.headers {
            name.zeroize();
            value.zeroize();
        }
        for (name, value) in &mut self.query {
            name.zeroize();
            value.zeroize();
        }
    }
}

fn exact_account<'a>(accounts: &'a AccountStore, account_id: &str) -> Option<&'a Account> {
    accounts
        .accounts
        .iter()
        .find(|account| account.id == account_id)
}

fn managed_auth(
    execution: &ProviderExecution,
    account: &Account,
    credential: crate::domain::accounts::managers::AccountCredential,
) -> AuthApplication {
    let mut auth = MaterializedAuth::default();
    if credential.credential_kind == CredentialKind::ApiKey
        && execution.plan.upstream_protocol == UpstreamProtocol::GeminiNative
    {
        auth.headers
            .push(("x-goog-api-key".to_string(), credential.value));
    } else {
        auth.headers.push((
            "authorization".to_string(),
            format!("Bearer {}", credential.value),
        ));
    }
    if execution.driver_is("oauth.openai_codex") {
        if let Some(account_id) = effective_codex_workspace_id(account) {
            auth.headers
                .push(("chatgpt-account-id".to_string(), account_id));
        }
        auth.headers.push((
            "originator".to_string(),
            crate::codex_identity::DEFAULT_CODEX_ORIGINATOR.to_string(),
        ));
        auth.headers.push((
            "version".to_string(),
            crate::codex_identity::configured_version(),
        ));
    }
    AuthApplication::Inject(auth)
}

fn managed_auth_is_protocol_owned(execution: &ProviderExecution) -> bool {
    matches!(
        execution.plan.driver_id.as_str(),
        "special.cursor"
            | "special.kiro"
            | "special.qoder_cosy"
            | "special.deepseek_account"
            | "special.copilot"
    )
}

fn static_auth(
    execution: &ProviderExecution,
    scheme: AuthScheme,
    secret: String,
) -> Result<AuthApplication, ProxyError> {
    let mut auth = MaterializedAuth::default();
    match scheme {
        AuthScheme::None => return Ok(AuthApplication::NoAuth(auth)),
        AuthScheme::ApiKey => {
            let header = if execution.plan.upstream_protocol == UpstreamProtocol::GeminiNative {
                "x-goog-api-key"
            } else {
                "x-api-key"
            };
            auth.headers.push((header.to_string(), secret));
        }
        AuthScheme::Bearer => auth
            .headers
            .push(("authorization".to_string(), format!("Bearer {secret}"))),
        AuthScheme::CustomHeader => {
            let name = execution
                .plan
                .driver_options
                .get("apiKeyField")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ProxyError::bad_request("custom_header auth requires apiKeyField")
                })?;
            validate_custom_auth_header(name)?;
            auth.headers.push((name.to_string(), secret));
        }
        AuthScheme::Query => {
            let name = execution
                .plan
                .driver_options
                .get("apiKeyField")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("key");
            auth.query.push((name.to_string(), secret));
        }
        AuthScheme::OAuth | AuthScheme::AwsSigV4 => {
            return Err(ProxyError::bad_request(format!(
                "static Provider cannot materialize {:?} authentication",
                scheme
            )));
        }
    }
    Ok(AuthApplication::Inject(auth))
}

fn static_credential_slot_allowed(execution: &ProviderExecution, slot: &str) -> bool {
    if matches!(
        execution.plan.driver_id.as_str(),
        "special.grok_web_session" | "special.perplexity_web_session"
    ) {
        return slot == crate::domain::providers::web_session::WEB_SESSION_CREDENTIAL_SLOT;
    }
    if slot == "/settingsConfig/apiKey" {
        return true;
    }
    match execution.stored.app {
        crate::domain::providers::model::AppKind::Claude => matches!(
            slot,
            "/settingsConfig/env/ANTHROPIC_AUTH_TOKEN"
                | "/settingsConfig/env/ANTHROPIC_API_KEY"
                | "/settingsConfig/env/API_KEY"
                | "/settingsConfig/env/AWS_BEARER_TOKEN_BEDROCK"
        ),
        crate::domain::providers::model::AppKind::Codex => matches!(
            slot,
            "/settingsConfig/auth/OPENAI_API_KEY"
                | "/settingsConfig/env/OPENAI_API_KEY"
                | "/settingsConfig/env/CODEX_API_KEY"
                | "/settingsConfig/env/API_KEY"
        ),
        crate::domain::providers::model::AppKind::Gemini => matches!(
            slot,
            "/settingsConfig/env/GEMINI_API_KEY"
                | "/settingsConfig/env/GOOGLE_API_KEY"
                | "/settingsConfig/env/API_KEY"
        ),
    }
}

fn credential_slot_priority(slot: &str) -> (u8, &str) {
    let priority = match slot {
        "/settingsConfig/apiKey" => 0,
        "/settingsConfig/auth/OPENAI_API_KEY" => 1,
        _ => 2,
    };
    (priority, slot)
}

fn validate_custom_auth_header(name: &str) -> Result<(), ProxyError> {
    crate::domain::providers::runtime::validate_custom_auth_header_name(name)
        .map(|_| ())
        .map_err(|error| ProxyError::bad_request(error.to_string()))
}

fn provider_secret(stored: &StoredProvider) -> Option<String> {
    stored
        .provider
        .settings_config
        .get("apiKey")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            setting(
                &stored.provider,
                &[
                    "ANTHROPIC_AUTH_TOKEN",
                    "ANTHROPIC_API_KEY",
                    "OPENAI_API_KEY",
                    "XAI_API_KEY",
                    "GROK_API_KEY",
                    "CODEX_API_KEY",
                    "GEMINI_API_KEY",
                    "GOOGLE_API_KEY",
                    "API_KEY",
                    "AWS_BEARER_TOKEN_BEDROCK",
                ],
            )
        })
        .or_else(|| codex_provider_api_key(&stored.provider))
}

fn runtime_model_is_body_field(execution: &ProviderExecution) -> bool {
    match execution.plan.upstream_protocol {
        UpstreamProtocol::AnthropicMessages
        | UpstreamProtocol::OpenAiChat
        | UpstreamProtocol::OpenAiResponses
        | UpstreamProtocol::Bedrock
        | UpstreamProtocol::Custom
        | UpstreamProtocol::Legacy => true,
        UpstreamProtocol::GeminiNative => false,
        UpstreamProtocol::Special => matches!(
            execution.plan.driver_id.as_str(),
            "special.cursor"
                | "special.copilot"
                | "special.kiro"
                | "special.deepseek_account"
                | "oauth.kimi_code"
        ),
    }
}

fn request_model(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("model")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
}

fn replace_request_model(body: &[u8], model: &str) -> Result<bytes::Bytes, ProxyError> {
    let mut value = serde_json::from_slice::<Value>(body).map_err(|error| {
        ProxyError::bad_request(format!("request body must be valid JSON: {error}"))
    })?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| ProxyError::bad_request("request body must be a JSON object"))?;
    object.insert("model".to_string(), Value::String(model.to_string()));
    serde_json::to_vec(&value)
        .map(bytes::Bytes::from)
        .map_err(|error| ProxyError::bad_request(format!("request encode failed: {error}")))
}

fn normalize_native_claude_context_1m_model(
    request: &mut AdapterRequest,
) -> Result<bool, ProxyError> {
    let requested = request
        .requested_model
        .clone()
        .or_else(|| request_model(&request.body));
    let context_1m_requested = requested
        .as_deref()
        .is_some_and(model_requests_claude_context_1m);
    if !context_1m_requested {
        return Ok(false);
    }

    let body_model = request_model(&request.body);
    if let Some((normalized, true)) = body_model
        .as_deref()
        .map(strip_bracketed_context_1m_suffixes)
    {
        request.body = replace_request_model(&request.body, &normalized)?;
    }
    normalize_request_model_field(&mut request.model);
    normalize_request_model_field(&mut request.actual_model);
    if request.actual_model_source.as_deref() == Some("request") {
        request.actual_model_source = Some("claude_context_1m_suffix".to_string());
    }
    Ok(true)
}

fn normalize_request_model_field(model: &mut Option<String>) {
    let Some(current) = model.as_deref() else {
        return;
    };
    let (normalized, changed) = strip_bracketed_context_1m_suffixes(current);
    if changed {
        *model = Some(normalized);
    }
}

fn strip_bracketed_context_1m_suffixes(model: &str) -> (String, bool) {
    let mut normalized = model.trim();
    let mut changed = false;
    while normalized.len() >= 4
        && normalized
            .get(normalized.len() - 4..)
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case("[1m]"))
    {
        normalized = normalized[..normalized.len() - 4].trim_end();
        changed = true;
    }
    (normalized.to_string(), changed)
}

fn model_requests_claude_context_1m(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    model.ends_with("[1m]") || model.ends_with("-1m")
}

fn canonical_auth_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization"
            | "x-api-key"
            | "api-key"
            | "x-goog-api-key"
            | "chatgpt-account-id"
            | "originator"
            | "version"
    )
}

fn replace_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    if let Some((_, current)) = headers
        .iter_mut()
        .find(|(current, _)| current.eq_ignore_ascii_case(name))
    {
        *current = value.to_string();
    } else {
        headers.push((name.to_string(), value.to_string()));
    }
}

fn now_ms_i64() -> i64 {
    crate::infra::time::now_ms().min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::providers::model::{AppKind, Provider, ProviderType};
    use crate::domain::providers::store::ProviderResourceMetadata;
    use serde_json::json;
    use sha2::{Digest, Sha256};

    fn execution_with_auth(
        auth_ref: RuntimeAuthRef,
        protocol: UpstreamProtocol,
        settings_config: Value,
        credential_generation: u64,
    ) -> ProviderExecution {
        let driver_id = match protocol {
            UpstreamProtocol::AnthropicMessages => "http.anthropic_messages",
            UpstreamProtocol::OpenAiChat => "http.openai_chat",
            UpstreamProtocol::OpenAiResponses => "http.openai_responses",
            UpstreamProtocol::GeminiNative => "http.gemini_native",
            _ => "legacy.frozen",
        };
        ProviderExecution {
            stored: StoredProvider {
                app: AppKind::Codex,
                provider: Provider {
                    id: "provider-auth".to_string(),
                    name: "Provider Auth".to_string(),
                    settings_config,
                    category: None,
                    meta: None,
                    extra: Default::default(),
                },
                provider_type: ProviderType::Codex,
                provider_type_id: ProviderType::Codex.as_str().to_string(),
                resource: ProviderResourceMetadata {
                    credential_generation,
                    ..Default::default()
                },
            },
            plan: Arc::new(ProviderRuntimePlan {
                provider_key: crate::domain::providers::registry::ProviderKey::new(
                    AppKind::Codex,
                    "provider-auth",
                )
                .unwrap(),
                provider_revision: 0,
                profile_id: crate::domain::providers::registry::ProfileId::parse(
                    "codex.custom_http",
                )
                .unwrap(),
                profile_schema_revision: 1,
                driver_id: crate::domain::providers::registry::DriverId::parse(driver_id).unwrap(),
                driver_contract_revision: 1,
                endpoint: "https://example.test".to_string(),
                upstream_protocol: protocol,
                outbound_identity_policy:
                    crate::domain::providers::registry::OutboundIdentityPolicy::CustomOverride,
                auth_ref,
                model_policy: RuntimeModelPolicy::Passthrough,
                coding_plan: None,
                test_model: None,
                probe_policy_fingerprint: "fixture".to_string(),
                aws_region: None,
                media_policy: None,
                transport_policy: Default::default(),
                extra_headers: Vec::new(),
                driver_options: Default::default(),
                configuration_state: RuntimeConfigurationState::Ready,
                warnings: vec![],
                runtime_fingerprint: "fixture".to_string(),
            }),
        }
    }

    fn typed_execution(
        app: AppKind,
        profile_id: &str,
        provider: Provider,
        accounts: &AccountStore,
        credential_generation: u64,
    ) -> ProviderExecution {
        let mut store = ProviderStore::default();
        let stored = store.upsert_with_resource(
            app,
            provider,
            ProviderResourceMetadata {
                profile_id: Some(
                    crate::domain::providers::registry::ProfileId::parse(profile_id).unwrap(),
                ),
                profile_schema_revision: Some(1),
                revision: 1,
                credential_generation,
                ..Default::default()
            },
        );
        let plan =
            crate::domain::providers::runtime::compile_runtime_plan(&stored, accounts).unwrap();
        assert_eq!(
            plan.configuration_state,
            RuntimeConfigurationState::Ready,
            "warnings={:?}",
            plan.warnings
        );
        assert_eq!(plan.profile_id.as_str(), profile_id);
        ProviderExecution {
            stored,
            plan: Arc::new(plan),
        }
    }

    fn typed_managed_provider(
        id: &str,
        provider_type: ProviderType,
        account_id: &str,
        settings_config: Value,
    ) -> Provider {
        Provider {
            id: id.to_string(),
            name: id.to_string(),
            settings_config,
            category: None,
            meta: Some(crate::domain::providers::model::ProviderMeta {
                provider_type: Some(provider_type.as_str().to_string()),
                auth_binding: Some(crate::domain::providers::model::AuthBinding {
                    source: Some("managed_account".to_string()),
                    auth_provider: Some(provider_type.as_str().to_string()),
                    account_id: Some(account_id.to_string()),
                    auth_identity_generation: Some(1),
                }),
                ..Default::default()
            }),
            extra: Default::default(),
        }
    }

    #[test]
    fn single_model_policy_rewrites_body_and_preserves_requested_model() {
        let execution = ProviderExecution {
            stored: StoredProvider {
                app: AppKind::Codex,
                provider: Provider {
                    id: "provider-a".to_string(),
                    name: "Provider A".to_string(),
                    settings_config: json!({}),
                    category: None,
                    meta: None,
                    extra: Default::default(),
                },
                provider_type: ProviderType::Codex,
                provider_type_id: ProviderType::Codex.as_str().to_string(),
                resource: ProviderResourceMetadata::default(),
            },
            plan: Arc::new(ProviderRuntimePlan {
                provider_key: crate::domain::providers::registry::ProviderKey::new(
                    AppKind::Codex,
                    "provider-a",
                )
                .unwrap(),
                provider_revision: 0,
                profile_id: crate::domain::providers::registry::ProfileId::parse(
                    "codex.openrouter",
                )
                .unwrap(),
                profile_schema_revision: 1,
                driver_id: crate::domain::providers::registry::DriverId::parse(
                    "http.openai_responses",
                )
                .unwrap(),
                driver_contract_revision: 1,
                endpoint: "https://example.test".to_string(),
                upstream_protocol: UpstreamProtocol::OpenAiResponses,
                outbound_identity_policy:
                    crate::domain::providers::registry::OutboundIdentityPolicy::ServerIdentity,
                auth_ref: RuntimeAuthRef::Missing,
                model_policy: RuntimeModelPolicy::Single {
                    upstream_model: "actual-model".to_string(),
                },
                coding_plan: None,
                test_model: None,
                probe_policy_fingerprint: "fixture".to_string(),
                aws_region: None,
                media_policy: None,
                transport_policy: Default::default(),
                extra_headers: Vec::new(),
                driver_options: Default::default(),
                configuration_state: RuntimeConfigurationState::Ready,
                warnings: vec![],
                runtime_fingerprint: "fixture".to_string(),
            }),
        };
        let mut request = AdapterRequest {
            body: bytes::Bytes::from_static(br#"{"model":"requested-model","input":[]}"#),
            upstream_endpoint: None,
            upstream_headers: vec![],
            model: Some("requested-model".to_string()),
            requested_model: Some("requested-model".to_string()),
            actual_model: None,
            actual_model_source: None,
            gemini_action: None,
            stream_requested: false,
            upstream_stream_requested: false,
            custom_tool_names: Default::default(),
            responses_tool_context: Default::default(),
            claude_tool_name_map: Default::default(),
        };

        execution.enforce_model_policy(&mut request).unwrap();

        assert_eq!(request.requested_model.as_deref(), Some("requested-model"));
        assert_eq!(request.actual_model.as_deref(), Some("actual-model"));
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["model"], "actual-model");
    }

    #[test]
    fn grok_test_contract_keeps_the_normalized_single_model_alias() {
        let mut execution = execution_with_auth(
            RuntimeAuthRef::ManagedAccount {
                account_id: "grok-account".to_string(),
                expected_provider_type: ProviderType::GrokOAuth,
                auth_identity_generation: 1,
            },
            UpstreamProtocol::OpenAiResponses,
            json!({}),
            1,
        );
        execution.stored.provider_type = ProviderType::GrokOAuth;
        execution.stored.provider_type_id = ProviderType::GrokOAuth.as_str().to_string();
        let plan = Arc::make_mut(&mut execution.plan);
        plan.driver_id =
            crate::domain::providers::registry::DriverId::parse("oauth.grok_responses").unwrap();
        plan.model_policy = RuntimeModelPolicy::Single {
            upstream_model: "grok-composer".to_string(),
        };

        let mut request = AdapterRequest {
            body: bytes::Bytes::from_static(br#"{"model":"requested-model","input":[]}"#),
            upstream_endpoint: None,
            upstream_headers: vec![],
            model: Some("requested-model".to_string()),
            requested_model: Some("requested-model".to_string()),
            actual_model: None,
            actual_model_source: None,
            gemini_action: None,
            stream_requested: false,
            upstream_stream_requested: false,
            custom_tool_names: Default::default(),
            responses_tool_context: Default::default(),
            claude_tool_name_map: Default::default(),
        };
        execution.enforce_model_policy(&mut request).unwrap();
        execution.finalize_request(&mut request).unwrap();
        let mut endpoint = execution
            .resolve_endpoint(ProxyRoute::CodexResponses, None, &request)
            .unwrap();
        execution
            .apply_test_forward_contract(
                ProxyRoute::CodexResponses,
                &mut request,
                &mut endpoint,
                &mut Vec::new(),
            )
            .unwrap();

        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["model"], "grok-composer-2.5-fast");
        assert_eq!(
            request.actual_model.as_deref(),
            Some("grok-composer-2.5-fast")
        );
    }

    #[test]
    fn codex_test_contract_applies_server_authoritative_fast_policy() {
        let mut execution = execution_with_auth(
            RuntimeAuthRef::ManagedAccount {
                account_id: "codex-account".to_string(),
                expected_provider_type: ProviderType::CodexOAuth,
                auth_identity_generation: 1,
            },
            UpstreamProtocol::OpenAiResponses,
            json!({}),
            1,
        );
        execution.stored.provider_type = ProviderType::CodexOAuth;
        execution.stored.provider_type_id = ProviderType::CodexOAuth.as_str().to_string();
        let plan = Arc::make_mut(&mut execution.plan);
        plan.driver_id =
            crate::domain::providers::registry::DriverId::parse("oauth.openai_codex").unwrap();
        plan.driver_options
            .insert("codexFastMode".to_string(), Value::Bool(true));

        let mut request = AdapterRequest {
            body: bytes::Bytes::from_static(
                b"{\"model\":\"gpt-5.4\",\"instructions\":\"  Keep caller policy.\\n\\nKeep spacing.  \",\"input\":\"ping\",\"reasoning\":{\"effort\":\"ultra\"},\"service_tier\":\"default\"}",
            ),
            upstream_endpoint: None,
            upstream_headers: vec![],
            model: Some("gpt-5.4".to_string()),
            requested_model: Some("gpt-5.4".to_string()),
            actual_model: Some("gpt-5.4".to_string()),
            actual_model_source: Some("request".to_string()),
            gemini_action: None,
            stream_requested: false,
            upstream_stream_requested: false,
            custom_tool_names: Default::default(),
            responses_tool_context: Default::default(),
            claude_tool_name_map: Default::default(),
        };
        let mut endpoint = "https://chatgpt.com/backend-api/codex/responses".to_string();

        execution
            .apply_test_forward_contract(
                ProxyRoute::CodexResponses,
                &mut request,
                &mut endpoint,
                &mut Vec::new(),
            )
            .unwrap();

        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
        assert_eq!(body["service_tier"], "priority");
        assert_eq!(
            body["instructions"],
            "  Keep caller policy.\n\nKeep spacing.  "
        );
        assert_eq!(body.pointer("/reasoning/effort"), Some(&json!("max")));
        assert!(!request.stream_requested);
        assert!(request.upstream_stream_requested);
    }

    #[test]
    fn codex_explicit_compact_uses_responses_transport_and_json_downstream() {
        let mut execution = execution_with_auth(
            RuntimeAuthRef::ManagedAccount {
                account_id: "codex-account".to_string(),
                expected_provider_type: ProviderType::CodexOAuth,
                auth_identity_generation: 1,
            },
            UpstreamProtocol::OpenAiResponses,
            json!({}),
            1,
        );
        execution.stored.provider_type = ProviderType::CodexOAuth;
        execution.stored.provider_type_id = ProviderType::CodexOAuth.as_str().to_string();
        let plan = Arc::make_mut(&mut execution.plan);
        plan.driver_id =
            crate::domain::providers::registry::DriverId::parse("oauth.openai_codex").unwrap();

        let mut request = AdapterRequest {
            body: bytes::Bytes::from_static(
                br#"{"type":"response.create","model":"gpt-5.5","input":"compact me"}"#,
            ),
            upstream_endpoint: None,
            upstream_headers: vec![],
            model: Some("gpt-5.5".to_string()),
            requested_model: Some("gpt-5.5".to_string()),
            actual_model: Some("gpt-5.5".to_string()),
            actual_model_source: Some("request".to_string()),
            gemini_action: None,
            stream_requested: false,
            upstream_stream_requested: false,
            custom_tool_names: Default::default(),
            responses_tool_context: Default::default(),
            claude_tool_name_map: Default::default(),
        };
        let mut endpoint = "https://chatgpt.com/backend-api/codex/responses/compact".to_string();

        execution
            .apply_test_forward_contract(
                ProxyRoute::CodexResponsesCompact,
                &mut request,
                &mut endpoint,
                &mut Vec::new(),
            )
            .unwrap();

        assert_eq!(endpoint, "https://chatgpt.com/backend-api/codex/responses");
        assert!(!request.stream_requested);
        assert!(request.upstream_stream_requested);
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert!(body.get("type").is_none());
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert_eq!(
            body.pointer("/input/1/type"),
            Some(&json!("compaction_trigger"))
        );
    }

    #[test]
    fn codex_ordinary_compaction_signal_stays_on_responses_endpoint() {
        let mut execution = execution_with_auth(
            RuntimeAuthRef::ManagedAccount {
                account_id: "codex-account".to_string(),
                expected_provider_type: ProviderType::CodexOAuth,
                auth_identity_generation: 1,
            },
            UpstreamProtocol::OpenAiResponses,
            json!({}),
            1,
        );
        execution.stored.provider_type = ProviderType::CodexOAuth;
        execution.stored.provider_type_id = ProviderType::CodexOAuth.as_str().to_string();
        Arc::make_mut(&mut execution.plan).driver_id =
            crate::domain::providers::registry::DriverId::parse("oauth.openai_codex").unwrap();
        let mut request = AdapterRequest {
            body: bytes::Bytes::from_static(
                br#"{"model":"gpt-5.5","input":[{"type":"compaction_trigger"},{"type":"message","role":"user","content":"keep"}]}"#,
            ),
            upstream_endpoint: None,
            upstream_headers: vec![],
            model: Some("gpt-5.5".to_string()),
            requested_model: Some("gpt-5.5".to_string()),
            actual_model: Some("gpt-5.5".to_string()),
            actual_model_source: Some("request".to_string()),
            gemini_action: None,
            stream_requested: true,
            upstream_stream_requested: false,
            custom_tool_names: Default::default(),
            responses_tool_context: Default::default(),
            claude_tool_name_map: Default::default(),
        };
        let mut endpoint = "https://chatgpt.com/backend-api/codex/responses".to_string();

        execution
            .apply_test_forward_contract(
                ProxyRoute::CodexResponses,
                &mut request,
                &mut endpoint,
                &mut Vec::new(),
            )
            .unwrap();

        assert_eq!(endpoint, "https://chatgpt.com/backend-api/codex/responses");
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.last().unwrap()["type"], "compaction_trigger");
    }

    #[test]
    fn codex_responses_lite_gate_uses_the_final_mapped_model() {
        let request_for = |model: &'static str| AdapterRequest {
            body: bytes::Bytes::from_static(match model {
                "gpt-5.6-sol" => br#"{"model":"gpt-5.6-sol","input":[]}"#,
                _ => br#"{"model":"gpt-5.4","input":[]}"#,
            }),
            upstream_endpoint: None,
            upstream_headers: vec![],
            model: Some(model.to_string()),
            requested_model: Some(model.to_string()),
            actual_model: None,
            actual_model_source: None,
            gemini_action: None,
            stream_requested: true,
            upstream_stream_requested: false,
            custom_tool_names: Default::default(),
            responses_tool_context: Default::default(),
            claude_tool_name_map: Default::default(),
        };
        let mut execution = execution_with_auth(
            RuntimeAuthRef::ManagedAccount {
                account_id: "codex-account".to_string(),
                expected_provider_type: ProviderType::CodexOAuth,
                auth_identity_generation: 1,
            },
            UpstreamProtocol::OpenAiResponses,
            json!({}),
            1,
        );
        execution.stored.provider_type = ProviderType::CodexOAuth;
        execution.stored.provider_type_id = ProviderType::CodexOAuth.as_str().to_string();
        let plan = Arc::make_mut(&mut execution.plan);
        plan.driver_id =
            crate::domain::providers::registry::DriverId::parse("oauth.openai_codex").unwrap();

        plan.model_policy = RuntimeModelPolicy::Single {
            upstream_model: "gpt-5.4".to_string(),
        };
        let mut lite_requested = request_for("gpt-5.6-sol");
        execution.enforce_model_policy(&mut lite_requested).unwrap();
        assert_eq!(lite_requested.actual_model.as_deref(), Some("gpt-5.4"));
        assert!(!execution.gate_openai_codex_responses_lite(&lite_requested, true));

        Arc::make_mut(&mut execution.plan).model_policy = RuntimeModelPolicy::Single {
            upstream_model: "gpt-5.6-sol".to_string(),
        };
        let mut legacy_requested = request_for("gpt-5.4");
        execution
            .enforce_model_policy(&mut legacy_requested)
            .unwrap();
        assert_eq!(
            legacy_requested.actual_model.as_deref(),
            Some("gpt-5.6-sol")
        );
        assert!(execution.gate_openai_codex_responses_lite(&legacy_requested, true));
    }

    #[test]
    fn openai_oauth_final_contract_is_shared_across_app_adapters() {
        let cases = [
            (
                AppKind::Claude,
                ProxyRoute::ClaudeMessages,
                bytes::Bytes::from_static(
                    br#"{"model":"claude-sonnet-4-6","system":"Claude policy","max_tokens":16,"messages":[{"role":"user","content":"ping"}],"output_config":{"effort":"max"},"stream":false}"#,
                ),
                None,
                "max",
                "Claude policy",
            ),
            (
                AppKind::Codex,
                ProxyRoute::CodexResponses,
                bytes::Bytes::from_static(
                    br#"{"model":"gpt-5.4","instructions":"Codex policy","input":"ping","reasoning":{"effort":"high"},"stream":false}"#,
                ),
                None,
                "high",
                "Codex policy",
            ),
            (
                AppKind::Gemini,
                ProxyRoute::Gemini,
                bytes::Bytes::from_static(
                    br#"{"systemInstruction":{"parts":[{"text":"Gemini policy"}]},"contents":[{"role":"user","parts":[{"text":"ping"}]}],"generationConfig":{"maxOutputTokens":16,"thinkingConfig":{"thinkingLevel":"xhigh"}}}"#,
                ),
                Some("models/gpt-5.4:generateContent"),
                "xhigh",
                "Gemini policy",
            ),
        ];

        for (app, route, body, gemini_path, expected_effort, expected_instructions) in cases {
            let mut execution = execution_with_auth(
                RuntimeAuthRef::ManagedAccount {
                    account_id: "codex-account".to_string(),
                    expected_provider_type: ProviderType::CodexOAuth,
                    auth_identity_generation: 1,
                },
                UpstreamProtocol::OpenAiResponses,
                json!({}),
                1,
            );
            execution.stored.app = app;
            execution.stored.provider_type = ProviderType::CodexOAuth;
            execution.stored.provider_type_id = ProviderType::CodexOAuth.as_str().to_string();
            let plan = Arc::make_mut(&mut execution.plan);
            plan.driver_id =
                crate::domain::providers::registry::DriverId::parse("oauth.openai_codex").unwrap();
            plan.model_policy = RuntimeModelPolicy::Single {
                upstream_model: "gpt-5.4".to_string(),
            };

            let stored = execution.runtime_stored_view();
            let adapter = adapters::adapter_for(app, ProviderType::CodexOAuth);
            let mut request = adapter
                .transform_request_for_route(body, &stored, route, gemini_path)
                .unwrap();
            execution.enforce_model_policy(&mut request).unwrap();
            execution.finalize_request(&mut request).unwrap();
            let mut endpoint = "https://chatgpt.com/backend-api/codex/responses".to_string();
            execution
                .apply_test_forward_contract(route, &mut request, &mut endpoint, &mut Vec::new())
                .unwrap();

            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["model"], "gpt-5.4", "app={}", app.as_str());
            assert_eq!(body["store"], false, "app={}", app.as_str());
            assert_eq!(body["stream"], true, "app={}", app.as_str());
            assert_eq!(
                body["instructions"],
                expected_instructions,
                "app={}",
                app.as_str()
            );
            assert_eq!(
                body.pointer("/reasoning/effort").and_then(Value::as_str),
                Some(expected_effort),
                "app={}",
                app.as_str()
            );
            assert!(!request.stream_requested, "app={}", app.as_str());
            assert!(request.upstream_stream_requested, "app={}", app.as_str());
        }
    }

    #[test]
    fn openai_oauth_final_contract_removes_client_fast_when_server_disables_it() {
        let mut execution = execution_with_auth(
            RuntimeAuthRef::ManagedAccount {
                account_id: "codex-account".to_string(),
                expected_provider_type: ProviderType::CodexOAuth,
                auth_identity_generation: 1,
            },
            UpstreamProtocol::OpenAiResponses,
            json!({}),
            1,
        );
        execution.stored.provider_type = ProviderType::CodexOAuth;
        execution.stored.provider_type_id = ProviderType::CodexOAuth.as_str().to_string();
        Arc::make_mut(&mut execution.plan).driver_id =
            crate::domain::providers::registry::DriverId::parse("oauth.openai_codex").unwrap();
        let mut request = AdapterRequest {
            body: bytes::Bytes::from_static(
                br#"{"model":"gpt-5.4","input":"ping","reasoning":{"effort":"high"},"service_tier":"priority","stream":false}"#,
            ),
            upstream_endpoint: None,
            upstream_headers: vec![],
            model: Some("gpt-5.4".to_string()),
            requested_model: Some("gpt-5.4".to_string()),
            actual_model: Some("gpt-5.4".to_string()),
            actual_model_source: Some("request".to_string()),
            gemini_action: None,
            stream_requested: false,
            upstream_stream_requested: false,
            custom_tool_names: Default::default(),
            responses_tool_context: Default::default(),
            claude_tool_name_map: Default::default(),
        };
        let intent = super::super::codex_request_policy::extract_intent_from_bytes(&request.body);
        let metadata = execution
            .apply_openai_codex_final_request_contract(
                ProxyRoute::CodexResponses,
                &mut request,
                None,
                false,
                &intent,
            )
            .unwrap();

        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert!(body.get("service_tier").is_none());
        assert_eq!(body.pointer("/reasoning/effort"), Some(&json!("high")));
        assert_eq!(metadata.requested_reasoning_effort.as_deref(), Some("high"));
        assert_eq!(metadata.effective_reasoning_effort.as_deref(), Some("high"));
        assert_eq!(metadata.client_service_tier.as_deref(), Some("priority"));
        assert_eq!(metadata.effective_service_tier, None);
        assert_eq!(
            metadata.service_tier_decision.as_deref(),
            Some("server_disabled")
        );
    }

    #[test]
    fn static_auth_placement_is_defined_by_the_runtime_scheme() {
        let api_key = execution_with_auth(
            RuntimeAuthRef::StaticCredential {
                auth_scheme: AuthScheme::ApiKey,
                slots: vec!["/settingsConfig/apiKey".to_string()],
                credential_generation: 2,
            },
            UpstreamProtocol::AnthropicMessages,
            json!({"apiKey": "secret-key"}),
            2,
        );
        let api_key_application = api_key.materialize_auth(&AccountStore::default()).unwrap();
        let api_key_auth = api_key_application.injected_values().unwrap();
        assert_eq!(
            api_key_auth.headers,
            vec![("x-api-key".to_string(), "secret-key".to_string())]
        );

        let bearer = execution_with_auth(
            RuntimeAuthRef::StaticCredential {
                auth_scheme: AuthScheme::Bearer,
                slots: vec!["/settingsConfig/apiKey".to_string()],
                credential_generation: 2,
            },
            UpstreamProtocol::OpenAiResponses,
            json!({"apiKey": "secret-key"}),
            2,
        );
        let bearer_application = bearer.materialize_auth(&AccountStore::default()).unwrap();
        let bearer_auth = bearer_application.injected_values().unwrap();
        assert_eq!(
            bearer_auth.headers,
            vec![("authorization".to_string(), "Bearer secret-key".to_string())]
        );
    }

    #[test]
    fn query_auth_replaces_stale_values_and_preserves_other_url_parts() {
        let mut execution = execution_with_auth(
            RuntimeAuthRef::StaticCredential {
                auth_scheme: AuthScheme::Query,
                slots: vec!["/settingsConfig/apiKey".to_string()],
                credential_generation: 2,
            },
            UpstreamProtocol::GeminiNative,
            json!({"apiKey": "fresh key/+"}),
            2,
        );
        Arc::make_mut(&mut execution.plan)
            .driver_options
            .insert("apiKeyField".to_string(), json!("key"));
        let application = execution
            .materialize_auth(&AccountStore::default())
            .unwrap();
        let mut headers = Vec::new();
        let mut url = "https://example.test/v1?key=stale&keep=a%2Fb&key=older#result".to_string();

        execution
            .apply_auth(&mut headers, &mut url, &application)
            .unwrap();

        let parsed = Url::parse(&url).unwrap();
        let pairs = parsed.query_pairs().into_owned().collect::<Vec<_>>();
        assert_eq!(
            pairs
                .iter()
                .filter(|(name, _)| name == "key")
                .cloned()
                .collect::<Vec<_>>(),
            vec![("key".to_string(), "fresh key/+".to_string())]
        );
        assert!(pairs.contains(&("keep".to_string(), "a/b".to_string())));
        assert_eq!(parsed.fragment(), Some("result"));
    }

    #[test]
    fn protocol_owned_auth_preserves_driver_authorization() {
        let execution = execution_with_auth(
            RuntimeAuthRef::AwsCredential {
                slots: vec![
                    "/settingsConfig/env/AWS_ACCESS_KEY_ID".to_string(),
                    "/settingsConfig/env/AWS_SECRET_ACCESS_KEY".to_string(),
                ],
                credential_generation: 2,
            },
            UpstreamProtocol::Bedrock,
            json!({}),
            2,
        );
        let application = execution
            .materialize_auth(&AccountStore::default())
            .unwrap();
        let mut headers = vec![
            (
                "authorization".to_string(),
                "AWS4-HMAC-SHA256 signed".to_string(),
            ),
            ("x-amz-date".to_string(), "20260802T000000Z".to_string()),
        ];
        let mut url = "https://bedrock.example/model/test/converse".to_string();

        execution
            .apply_auth(&mut headers, &mut url, &application)
            .unwrap();

        assert_eq!(
            headers[0],
            (
                "authorization".to_string(),
                "AWS4-HMAC-SHA256 signed".to_string()
            )
        );
    }

    #[test]
    fn typed_bedrock_pipeline_converts_and_signs_only_at_protocol_boundary() {
        let accounts = AccountStore::default();
        let execution = typed_execution(
            AppKind::Claude,
            "claude.aws_bedrock_aksk",
            Provider {
                id: "typed-bedrock".to_string(),
                name: "Typed Bedrock".to_string(),
                settings_config: json!({
                    "env": {
                        "ANTHROPIC_BASE_URL": "https://bedrock-runtime.us-west-2.amazonaws.com",
                        "AWS_REGION": "us-west-2",
                        "AWS_ACCESS_KEY_ID": "AKIA1234567890ABCD",
                        "AWS_SECRET_ACCESS_KEY": "test-secret",
                        "AWS_SESSION_TOKEN": "test-session"
                    }
                }),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            &accounts,
            1,
        );
        assert_eq!(execution.plan.driver_id.as_str(), "aws.bedrock_sigv4");

        let stored = execution.runtime_stored_view();
        let adapter = adapters::adapter_for(AppKind::Claude, stored.provider_type);
        let mut request = adapter
            .transform_request_for_route(
                Bytes::from_static(
                    br#"{"model":"global.anthropic.claude-opus-4-8","max_tokens":128,"temperature":0.25,"system":[{"type":"text","text":"keep this rule"}],"messages":[{"role":"user","content":[{"type":"text","text":"hello bedrock"}]}]}"#,
                ),
                &stored,
                ProxyRoute::ClaudeMessages,
                None,
            )
            .unwrap();
        execution.enforce_model_policy(&mut request).unwrap();
        let anthropic_body = request.body.clone();

        execution.finalize_request(&mut request).unwrap();

        assert_eq!(request.body, anthropic_body);
        assert!(request.upstream_endpoint.is_none());
        assert!(request.upstream_headers.is_empty());

        let mut endpoint = execution
            .resolve_endpoint(ProxyRoute::ClaudeMessages, None, &request)
            .unwrap();
        let mut headers = adapter
            .build_headers(AppKind::Claude, &stored, &accounts)
            .unwrap()
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect::<Vec<_>>();
        headers.extend(
            request
                .upstream_headers
                .iter()
                .map(|(name, value)| (name.to_string(), value.clone())),
        );
        let auth = execution.materialize_auth(&accounts).unwrap();
        execution
            .apply_auth(&mut headers, &mut endpoint, &auth)
            .unwrap();
        apply_account_header_overrides(&mut headers, &stored, &accounts).unwrap();
        execution.finalize_outbound_identity(&mut headers).unwrap();
        execution
            .finalize_protocol_auth(&accounts, &mut request, &mut endpoint, &mut headers)
            .unwrap();

        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(
            body.pointer("/messages/0/content/0/text"),
            Some(&json!("hello bedrock"))
        );
        assert_eq!(
            body.pointer("/system/0/text"),
            Some(&json!("keep this rule"))
        );
        assert_eq!(
            body.pointer("/inferenceConfig/maxTokens"),
            Some(&json!(128))
        );
        assert_eq!(
            body.pointer("/inferenceConfig/temperature"),
            Some(&json!(0.25))
        );
        assert!(body.get("max_tokens").is_none());
        assert!(endpoint.ends_with("/converse"));

        for name in [
            "authorization",
            "content-type",
            "host",
            "x-amz-content-sha256",
            "x-amz-date",
            "x-amz-security-token",
        ] {
            assert_eq!(
                headers
                    .iter()
                    .filter(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
                    .count(),
                1,
                "header={name}"
            );
        }
        let payload_hash = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("x-amz-content-sha256"))
            .map(|(_, value)| value.as_str())
            .unwrap();
        assert_eq!(payload_hash, hex::encode(Sha256::digest(&request.body)));
    }

    #[test]
    fn typed_copilot_protocol_auth_survives_final_header_assembly_once() {
        let account: Account = serde_json::from_value(json!({
            "id": "copilot-account",
            "providerType": "github_copilot",
            "authIdentityGeneration": 1,
            "accessToken": "cached-copilot-token",
            "refreshToken": "github-token",
            "expiresAt": 1,
            "tokenType": "Bearer"
        }))
        .unwrap();
        let accounts = AccountStore {
            accounts: vec![account],
            ..Default::default()
        };
        let execution = typed_execution(
            AppKind::Claude,
            "claude.github_copilot",
            typed_managed_provider(
                "typed-copilot",
                ProviderType::GitHubCopilot,
                "copilot-account",
                json!({}),
            ),
            &accounts,
            0,
        );
        assert_eq!(execution.plan.driver_id.as_str(), "special.copilot");

        let application = execution.materialize_auth(&accounts).unwrap();
        assert!(matches!(application, AuthApplication::ProtocolOwned));
        let mut target_headers = vec![
            (
                "authorization".to_string(),
                "Bearer cached-copilot-token".to_string(),
            ),
            ("user-agent".to_string(), "downstream-client/1".to_string()),
        ];
        let mut endpoint = "https://api.githubcopilot.com/chat/completions".to_string();
        execution
            .apply_auth(&mut target_headers, &mut endpoint, &application)
            .unwrap();
        execution
            .finalize_outbound_identity(&mut target_headers)
            .unwrap();

        let mut client_headers = HeaderMap::new();
        client_headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer untrusted-downstream-token"),
        );
        let headers = super::super::outbound_request::assemble_headers(
            &client_headers,
            &target_headers,
            "*/*",
            "application/json",
        )
        .unwrap();

        assert_eq!(headers.get_all("authorization").iter().count(), 1);
        assert_eq!(
            headers
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer cached-copilot-token")
        );
        assert_eq!(
            headers
                .get("user-agent")
                .and_then(|value| value.to_str().ok()),
            Some(crate::provider_identity::COPILOT_USER_AGENT)
        );
    }

    #[test]
    fn typed_kiro_protocol_auth_survives_final_header_assembly_once() {
        let account: Account = serde_json::from_value(json!({
            "id": "kiro-account",
            "providerType": "kiro_oauth",
            "authIdentityGeneration": 1,
            "accessToken": "kiro-access-token",
            "refreshToken": "kiro-refresh-token",
            "tokenType": "Bearer",
            "profile": {
                "profileArn": "arn:aws:codewhisperer:us-east-1:123456789012:profile/test",
                "apiRegion": "us-east-1",
                "machineId": "machine-test"
            }
        }))
        .unwrap();
        let accounts = AccountStore {
            accounts: vec![account.clone()],
            ..Default::default()
        };
        let execution = typed_execution(
            AppKind::Claude,
            "claude.kiro_oauth",
            typed_managed_provider(
                "typed-kiro",
                ProviderType::KiroOAuth,
                "kiro-account",
                json!({}),
            ),
            &accounts,
            0,
        );
        assert_eq!(execution.plan.driver_id.as_str(), "special.kiro");

        let application = execution.materialize_auth(&accounts).unwrap();
        assert!(matches!(application, AuthApplication::ProtocolOwned));
        let prepared = super::super::kiro::prepare_kiro_request(
            &account,
            &json!({
                "model": "claude-sonnet-4-8",
                "max_tokens": 32,
                "messages": [{"role": "user", "content": "ping"}]
            }),
        )
        .unwrap();
        let mut target_headers = prepared
            .headers
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect::<Vec<_>>();
        let mut endpoint = prepared.url;
        execution
            .apply_auth(&mut target_headers, &mut endpoint, &application)
            .unwrap();
        execution
            .finalize_outbound_identity(&mut target_headers)
            .unwrap();

        let mut client_headers = HeaderMap::new();
        client_headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer untrusted-downstream-token"),
        );
        let headers = super::super::outbound_request::assemble_headers(
            &client_headers,
            &target_headers,
            "*/*",
            "application/json",
        )
        .unwrap();

        assert_eq!(headers.get_all("authorization").iter().count(), 1);
        assert_eq!(
            headers
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer kiro-access-token")
        );
        assert_eq!(
            headers
                .get("user-agent")
                .and_then(|value| value.to_str().ok()),
            Some("aws-sdk-js/1.0.34 KiroIDE-2.3.0")
        );
    }

    #[test]
    fn typed_gemini_api_key_is_finalized_as_one_header() {
        let execution = typed_execution(
            AppKind::Gemini,
            "gemini.google_api_key",
            Provider {
                id: "typed-gemini".to_string(),
                name: "Typed Gemini".to_string(),
                settings_config: json!({
                    "env": {"GEMINI_API_KEY": "typed-gemini-key"}
                }),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            &AccountStore::default(),
            3,
        );
        assert_eq!(execution.plan.driver_id.as_str(), "http.gemini_native");

        let application = execution
            .materialize_auth(&AccountStore::default())
            .unwrap();
        let mut target_headers = vec![
            ("X-Goog-Api-Key".to_string(), "stale-key".to_string()),
            ("x-goog-api-key".to_string(), "older-key".to_string()),
            (
                "authorization".to_string(),
                "Bearer downstream-token".to_string(),
            ),
        ];
        let mut endpoint = execution.plan.endpoint.clone();
        execution
            .apply_auth(&mut target_headers, &mut endpoint, &application)
            .unwrap();
        execution
            .finalize_outbound_identity(&mut target_headers)
            .unwrap();
        let headers = super::super::outbound_request::assemble_headers(
            &HeaderMap::new(),
            &target_headers,
            "*/*",
            "application/json",
        )
        .unwrap();

        assert_eq!(headers.get_all("x-goog-api-key").iter().count(), 1);
        assert_eq!(
            headers
                .get("x-goog-api-key")
                .and_then(|value| value.to_str().ok()),
            Some("typed-gemini-key")
        );
        assert!(headers.get("authorization").is_none());
    }

    #[test]
    fn static_auth_reads_only_declared_runtime_slots() {
        let execution = execution_with_auth(
            RuntimeAuthRef::StaticCredential {
                auth_scheme: AuthScheme::Bearer,
                slots: vec!["/settingsConfig/env/OPENAI_API_KEY".to_string()],
                credential_generation: 2,
            },
            UpstreamProtocol::OpenAiResponses,
            json!({
                "apiKey": "unrelated-canonical-key",
                "env": {"OPENAI_API_KEY": "declared-key"}
            }),
            2,
        );

        let application = execution
            .materialize_auth(&AccountStore::default())
            .unwrap();

        assert_eq!(
            application.injected_values().unwrap().headers,
            vec![(
                "authorization".to_string(),
                "Bearer declared-key".to_string()
            )]
        );
    }

    #[test]
    fn static_auth_rejects_conflicting_runtime_slots() {
        let execution = execution_with_auth(
            RuntimeAuthRef::StaticCredential {
                auth_scheme: AuthScheme::Bearer,
                slots: vec![
                    "/settingsConfig/apiKey".to_string(),
                    "/settingsConfig/env/OPENAI_API_KEY".to_string(),
                ],
                credential_generation: 2,
            },
            UpstreamProtocol::OpenAiResponses,
            json!({
                "apiKey": "canonical-key",
                "env": {"OPENAI_API_KEY": "legacy-key"}
            }),
            2,
        );

        let error = execution
            .materialize_auth(&AccountStore::default())
            .unwrap_err();

        assert!(error.message.contains("conflicting credentials"));
    }

    #[test]
    fn materialization_rejects_stale_credential_and_account_identity_generations() {
        let stale_credential = execution_with_auth(
            RuntimeAuthRef::StaticCredential {
                auth_scheme: AuthScheme::Bearer,
                slots: vec!["/settingsConfig/apiKey".to_string()],
                credential_generation: 1,
            },
            UpstreamProtocol::OpenAiResponses,
            json!({"apiKey": "secret-key"}),
            2,
        );
        let error = stale_credential
            .materialize_auth(&AccountStore::default())
            .unwrap_err();
        assert_eq!(error.status, StatusCode::CONFLICT);

        let mut stale_identity = execution_with_auth(
            RuntimeAuthRef::ManagedAccount {
                account_id: "account-1".to_string(),
                expected_provider_type: ProviderType::CodexOAuth,
                auth_identity_generation: 1,
            },
            UpstreamProtocol::OpenAiResponses,
            json!({}),
            0,
        );
        stale_identity.stored.provider_type = ProviderType::CodexOAuth;
        let account = serde_json::from_value(json!({
            "id": "account-1",
            "providerType": "codex_oauth",
            "authIdentityGeneration": 2,
            "accessToken": "access-token"
        }))
        .unwrap();
        let error = stale_identity
            .materialize_auth(&AccountStore {
                accounts: vec![account],
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(error.status, StatusCode::CONFLICT);
    }

    #[test]
    fn materialization_uses_the_explicit_codex_account_binding() {
        let mut execution = execution_with_auth(
            RuntimeAuthRef::ManagedAccount {
                account_id: "account-1".to_string(),
                expected_provider_type: ProviderType::CodexOAuth,
                auth_identity_generation: 1,
            },
            UpstreamProtocol::OpenAiResponses,
            json!({}),
            0,
        );
        execution.stored.provider_type = ProviderType::CodexOAuth;
        let mut accounts = AccountStore::default();
        for account_id in ["account-1", "account-2"] {
            accounts.upsert(
                serde_json::from_value(json!({
                    "id": account_id,
                    "providerType": "codex_oauth",
                    "accessToken": format!("access-{account_id}")
                }))
                .unwrap(),
            );
        }
        accounts
            .select_active_codex_oauth_account("account-2")
            .unwrap();

        let auth = execution.materialize_auth(&accounts).unwrap();
        let injected = auth.injected_values().unwrap();
        assert!(injected.headers.iter().any(|(name, value)| {
            name == "authorization" && value == "Bearer access-account-1"
        }));
        assert!(!injected
            .headers
            .iter()
            .any(|(_, value)| value.contains("account-2")));
    }

    #[test]
    fn managed_account_target_preserves_typed_and_legacy_oauth_bindings() {
        let typed = execution_with_auth(
            RuntimeAuthRef::ManagedAccount {
                account_id: "typed-account".to_string(),
                expected_provider_type: ProviderType::ClaudeOAuth,
                auth_identity_generation: 1,
            },
            UpstreamProtocol::AnthropicMessages,
            json!({}),
            0,
        );
        assert_eq!(
            typed.managed_account_target(),
            Some((ProviderType::ClaudeOAuth, "typed-account"))
        );

        let mut legacy = execution_with_auth(
            RuntimeAuthRef::Legacy {
                account_id: Some("legacy-account".to_string()),
                credential_generation: 0,
            },
            UpstreamProtocol::AnthropicMessages,
            json!({}),
            0,
        );
        legacy.stored.app = AppKind::Claude;
        legacy.stored.provider_type = ProviderType::ClaudeOAuth;
        legacy.plan = Arc::new(ProviderRuntimePlan {
            configuration_state: RuntimeConfigurationState::LegacyCompat,
            ..legacy.plan.as_ref().clone()
        });
        assert_eq!(
            legacy.managed_account_target(),
            Some((ProviderType::ClaudeOAuth, "legacy-account"))
        );

        legacy.stored.provider.settings_config =
            json!({"env": {"ANTHROPIC_AUTH_TOKEN": "provider-secret"}});
        assert_eq!(
            legacy.managed_account_target(),
            Some((ProviderType::ClaudeOAuth, "legacy-account"))
        );

        legacy.stored.app = AppKind::Codex;
        legacy.stored.provider_type = ProviderType::GrokOAuth;
        legacy.stored.provider.settings_config =
            json!({"env": {"OPENAI_API_KEY": "stale-provider-secret"}});
        assert_eq!(
            legacy.managed_account_target(),
            Some((ProviderType::GrokOAuth, "legacy-account"))
        );
    }

    #[test]
    fn grok_account_headers_cannot_override_proxy_protocol_identity() {
        for name in [
            "x-xai-token-auth",
            "X-Grok-Client-Identifier",
            "x-grok-client-version",
            "x-grok-client-surface",
            "x-authenticateresponse",
            "x-grok-conv-id",
            "x-grok-cache-identity",
            "x-grok-turn-idx",
        ] {
            assert!(account_header_override_blocked(
                name,
                ProviderType::GrokOAuth
            ));
        }
        assert!(!account_header_override_blocked(
            "x-enterprise-sso",
            ProviderType::GrokOAuth
        ));
    }

    #[test]
    fn unbound_legacy_managed_oauth_is_not_ready_and_never_selects_first_account() {
        let mut legacy = execution_with_auth(
            RuntimeAuthRef::Legacy {
                account_id: None,
                credential_generation: 0,
            },
            UpstreamProtocol::OpenAiResponses,
            json!({}),
            0,
        );
        legacy.stored.provider_type = ProviderType::CodexOAuth;
        legacy.plan = Arc::new(ProviderRuntimePlan {
            configuration_state: RuntimeConfigurationState::LegacyCompat,
            ..legacy.plan.as_ref().clone()
        });

        let error = legacy.ensure_ready().unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("must explicitly bind"));
        assert_eq!(legacy.managed_account_target(), None);
    }

    #[test]
    fn unbound_legacy_grok_is_not_ready_even_with_a_static_secret() {
        let mut legacy = execution_with_auth(
            RuntimeAuthRef::Legacy {
                account_id: None,
                credential_generation: 1,
            },
            UpstreamProtocol::OpenAiResponses,
            json!({"env": {"OPENAI_API_KEY": "legacy-secret"}}),
            1,
        );
        legacy.stored.provider_type = ProviderType::GrokOAuth;
        legacy.stored.provider_type_id = ProviderType::GrokOAuth.as_str().to_string();
        legacy.plan = Arc::new(ProviderRuntimePlan {
            configuration_state: RuntimeConfigurationState::LegacyCompat,
            ..legacy.plan.as_ref().clone()
        });

        let error = legacy.ensure_ready().unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("grok_oauth managed account"));
        assert_eq!(legacy.managed_account_target(), None);
    }

    #[test]
    fn native_claude_context_suffix_normalizes_brackets_but_preserves_entity_models() {
        let mut request = AdapterRequest {
            body: bytes::Bytes::from_static(
                br#"{"model":"Claude-Opus-4-6[1m][1M]","messages":[]}"#,
            ),
            upstream_endpoint: None,
            upstream_headers: Vec::new(),
            model: Some("Claude-Opus-4-6[1m][1M]".to_string()),
            requested_model: Some("Claude-Opus-4-6[1m][1M]".to_string()),
            actual_model: Some("Claude-Opus-4-6[1m][1M]".to_string()),
            actual_model_source: Some("request".to_string()),
            gemini_action: None,
            stream_requested: false,
            upstream_stream_requested: false,
            custom_tool_names: Default::default(),
            responses_tool_context: Default::default(),
            claude_tool_name_map: Default::default(),
        };

        assert!(normalize_native_claude_context_1m_model(&mut request).unwrap());
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["model"], "Claude-Opus-4-6");
        assert_eq!(
            request.requested_model.as_deref(),
            Some("Claude-Opus-4-6[1m][1M]")
        );
        assert_eq!(request.model.as_deref(), Some("Claude-Opus-4-6"));
        assert_eq!(request.actual_model.as_deref(), Some("Claude-Opus-4-6"));
        assert_eq!(
            request.actual_model_source.as_deref(),
            Some("claude_context_1m_suffix")
        );

        let mut entity_model = AdapterRequest {
            body: bytes::Bytes::from_static(br#"{"model":"claude-sonnet-4-6-1m","messages":[]}"#),
            upstream_endpoint: None,
            upstream_headers: Vec::new(),
            model: Some("claude-sonnet-4-6-1m".to_string()),
            requested_model: Some("claude-sonnet-4-6-1m".to_string()),
            actual_model: Some("claude-sonnet-4-6-1m".to_string()),
            actual_model_source: Some("request".to_string()),
            gemini_action: None,
            stream_requested: false,
            upstream_stream_requested: false,
            custom_tool_names: Default::default(),
            responses_tool_context: Default::default(),
            claude_tool_name_map: Default::default(),
        };
        assert!(normalize_native_claude_context_1m_model(&mut entity_model).unwrap());
        let body: Value = serde_json::from_slice(&entity_model.body).unwrap();
        assert_eq!(body["model"], "claude-sonnet-4-6-1m");
        assert_eq!(
            entity_model.actual_model.as_deref(),
            Some("claude-sonnet-4-6-1m")
        );
    }

    #[test]
    fn typed_claude_request_signs_the_normalized_body_once() {
        let mut execution = execution_with_auth(
            RuntimeAuthRef::ManagedAccount {
                account_id: "claude-account".to_string(),
                expected_provider_type: ProviderType::ClaudeOAuth,
                auth_identity_generation: 1,
            },
            UpstreamProtocol::AnthropicMessages,
            json!({"env": {"ANTHROPIC_AUTH_TOKEN": "stale-provider-secret"}}),
            0,
        );
        execution.stored.app = AppKind::Claude;
        execution.stored.provider.id = "typed-claude".to_string();
        execution.stored.provider_type = ProviderType::ClaudeOAuth;
        execution.stored.provider_type_id = ProviderType::ClaudeOAuth.as_str().to_string();
        execution.stored.provider.meta = Some(crate::domain::providers::model::ProviderMeta {
            auth_binding: Some(crate::domain::providers::model::AuthBinding {
                source: Some("account".to_string()),
                auth_provider: Some("claude_oauth".to_string()),
                account_id: Some("claude-account".to_string()),
                auth_identity_generation: Some(1),
            }),
            ..Default::default()
        });
        execution.plan = Arc::new(ProviderRuntimePlan {
            provider_key: crate::domain::providers::registry::ProviderKey::new(
                AppKind::Claude,
                "typed-claude",
            )
            .unwrap(),
            profile_id: crate::domain::providers::registry::ProfileId::parse(
                "claude.official_oauth",
            )
            .unwrap(),
            driver_id: crate::domain::providers::registry::DriverId::parse("oauth.claude_messages")
                .unwrap(),
            endpoint: "https://api.anthropic.com".to_string(),
            ..execution.plan.as_ref().clone()
        });
        let account = serde_json::from_value(json!({
            "id": "claude-account",
            "providerType": "claude_oauth",
            "authIdentityGeneration": 1,
            "accessToken": "typed-access-token",
            "tokenType": "Bearer"
        }))
        .unwrap();
        let accounts = AccountStore {
            accounts: vec![account],
            ..Default::default()
        };
        let mut client_headers = HeaderMap::new();
        client_headers.insert(
            "x-session-id",
            HeaderValue::from_static("stable-session-id"),
        );
        let prepared = execution
            .prepare_claude_request(
                bytes::Bytes::from_static(
                    br#"{"model":"claude-sonnet-4-6[1m][1M]","max_tokens":16,"messages":[{"role":"user","content":"ping"}],"tools":[{"name":"read","input_schema":{"type":"object"}}]}"#,
                ),
                ProxyRoute::ClaudeMessages,
                &client_headers,
                &accounts,
                None,
            )
            .unwrap();

        assert_eq!(
            prepared.endpoint,
            "https://api.anthropic.com/v1/messages?beta=true"
        );
        let body: Value = serde_json::from_slice(&prepared.adapter_request.body).unwrap();
        assert_eq!(body["model"], "claude-sonnet-4-6");
        assert_eq!(body.pointer("/tools/0/name"), Some(&json!("Read")));
        assert_eq!(
            prepared
                .adapter_request
                .claude_tool_name_map
                .get("read")
                .map(String::as_str),
            Some("read")
        );
        let signed_text = body["system"][0]["text"].as_str().unwrap();
        assert!(signed_text.contains("cch="));
        assert!(!signed_text.contains("cch=00000"));
        assert_eq!(
            prepared
                .headers
                .iter()
                .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                .count(),
            1
        );
        assert!(prepared.headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("authorization") && value == "Bearer typed-access-token"
        }));
        assert!(!prepared
            .headers
            .iter()
            .any(|(_, value)| value.contains("stale-provider-secret")));
        assert!(prepared.headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("anthropic-beta") && value.contains("context-1m")
        }));

        let mut signed_again = prepared.adapter_request.body.clone();
        let mut endpoint = "https://api.anthropic.com/v1/messages".to_string();
        super::super::claude_oauth::apply_forward_contract(
            &mut endpoint,
            &mut signed_again,
            &client_headers,
            "claude-account",
            true,
            None,
        )
        .unwrap();
        assert_eq!(signed_again, prepared.adapter_request.body);
    }

    #[test]
    fn typed_kimi_surfaces_project_app_protocols_and_exact_coding_endpoints() {
        let account: Account = serde_json::from_value(json!({
            "id": "kimi-contract-account",
            "providerType": "kimi_code",
            "authIdentityGeneration": 1,
            "tokenRefreshGeneration": 3,
            "accessToken": "kimi-contract-access",
            "refreshToken": "kimi-contract-refresh",
            "tokenType": "Bearer",
            "expiresAt": i64::MAX / 2,
            "profile": {
                "userId": "kimi-contract-user",
                "kimiDevice": {
                    "deviceId": "kimi-contract-device",
                    "deviceName": "contract-fixture",
                    "deviceModel": "contract-fixture",
                    "osVersion": "contract-fixture"
                }
            }
        }))
        .unwrap();
        let accounts = AccountStore {
            accounts: vec![account],
            ..Default::default()
        };

        struct Case {
            app: AppKind,
            profile_id: &'static str,
            route: ProxyRoute,
            gemini_path: Option<&'static str>,
            body: &'static [u8],
            api_format: &'static str,
            endpoint: &'static str,
        }

        for (case_index, case) in [
            Case {
                app: AppKind::Claude,
                profile_id: "claude.kimi_code",
                route: ProxyRoute::ClaudeMessages,
                gemini_path: None,
                body: br#"{"model":"claude-sonnet-5","max_tokens":32,"messages":[{"role":"user","content":"ping"}]}"#,
                api_format: "anthropic",
                endpoint: "https://api.kimi.com/coding/v1/messages?beta=true",
            },
            Case {
                app: AppKind::Claude,
                profile_id: "claude.kimi_code",
                route: ProxyRoute::ClaudeCountTokens,
                gemini_path: None,
                body: br#"{"model":"claude-sonnet-5","messages":[{"role":"user","content":"count"}]}"#,
                api_format: "anthropic",
                endpoint: "https://api.kimi.com/coding/v1/messages/count_tokens?beta=true",
            },
            Case {
                app: AppKind::Codex,
                profile_id: "codex.kimi_code",
                route: ProxyRoute::CodexResponses,
                gemini_path: None,
                body: br#"{"model":"gpt-5.5","input":"ping","stream":false}"#,
                api_format: "openai_chat",
                endpoint: "https://api.kimi.com/coding/v1/chat/completions",
            },
            Case {
                app: AppKind::Codex,
                profile_id: "codex.kimi_code",
                route: ProxyRoute::CodexChatCompletions,
                gemini_path: None,
                body: br#"{"model":"gpt-5.5","messages":[{"role":"user","content":"ping"}],"stream":false}"#,
                api_format: "openai_chat",
                endpoint: "https://api.kimi.com/coding/v1/chat/completions",
            },
            Case {
                app: AppKind::Gemini,
                profile_id: "gemini.kimi_code",
                route: ProxyRoute::Gemini,
                gemini_path: Some("models/gemini-3.5-flash:generateContent"),
                body: br#"{"contents":[{"role":"user","parts":[{"text":"ping"}]}]}"#,
                api_format: "openai_chat",
                endpoint: "https://api.kimi.com/coding/v1/chat/completions",
            },
        ]
        .into_iter()
        .enumerate()
        {
            let execution = typed_execution(
                case.app,
                case.profile_id,
                typed_managed_provider(
                    &format!("typed-kimi-{case_index}"),
                    ProviderType::KimiCode,
                    "kimi-contract-account",
                    json!({}),
                ),
                &accounts,
                0,
            );
            assert_eq!(execution.plan.driver_id.as_str(), "oauth.kimi_code");
            assert_eq!(execution.plan.driver_contract_revision, 3);
            assert_eq!(execution.plan.upstream_protocol, UpstreamProtocol::Special);
            assert_eq!(
                execution.managed_account_identity_target(),
                Some((ProviderType::KimiCode, "kimi-contract-account", 1))
            );

            let stored = execution.runtime_stored_view();
            assert_eq!(
                stored
                    .provider
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.api_format.as_deref()),
                Some(case.api_format)
            );
            let adapter = adapters::adapter_for(case.app, ProviderType::KimiCode);
            let mut request = adapter
                .transform_request_for_route(
                    Bytes::copy_from_slice(case.body),
                    &stored,
                    case.route,
                    case.gemini_path,
                )
                .unwrap();
            execution.enforce_model_policy(&mut request).unwrap();
            execution.finalize_request(&mut request).unwrap();
            let mut endpoint = execution
                .resolve_endpoint(
                    case.route,
                    case.gemini_path.map(str::to_string),
                    &request,
                )
                .unwrap();
            assert_eq!(endpoint, case.endpoint);
            assert_eq!(request.actual_model.as_deref(), Some("kimi-for-coding"));
            assert_eq!(
                serde_json::from_slice::<Value>(&request.body).unwrap()["model"],
                json!("kimi-for-coding")
            );

            let mut headers = adapter
                .build_headers(case.app, &stored, &accounts)
                .unwrap()
                .into_iter()
                .map(|(name, value)| (name.to_string(), value))
                .collect::<Vec<_>>();
            let auth = execution.materialize_auth(&accounts).unwrap();
            execution
                .apply_auth(&mut headers, &mut endpoint, &auth)
                .unwrap();
            execution.finalize_outbound_identity(&mut headers).unwrap();
            execution
                .finalize_protocol_auth(&accounts, &mut request, &mut endpoint, &mut headers)
                .unwrap();
            for (name, value) in [
                ("authorization", "Bearer kimi-contract-access"),
                ("x-msh-platform", "kimi_cli"),
                ("x-msh-device-id", "kimi-contract-device"),
                ("user-agent", crate::domain::kimi_cli::KIMI_USER_AGENT),
            ] {
                assert!(headers.iter().any(|(candidate, candidate_value)| {
                    candidate.eq_ignore_ascii_case(name) && candidate_value == value
                }), "app={:?} route={:?} missing {name}", case.app, case.route);
            }
        }
    }

    #[test]
    fn custom_protocol_projection_uses_runtime_driver_not_legacy_provider_type() {
        let execution = execution_with_auth(
            RuntimeAuthRef::CustomCredential {
                auth_scheme: AuthScheme::None,
                slots: vec![],
                credential_generation: 0,
            },
            UpstreamProtocol::AnthropicMessages,
            json!({}),
            0,
        );

        let projected = execution.runtime_stored_view();

        assert_eq!(
            projected
                .provider
                .meta
                .as_ref()
                .and_then(|meta| meta.api_format.as_deref()),
            Some("anthropic")
        );
    }

    #[test]
    fn custom_extra_headers_are_materialized_from_declared_secret_slots() {
        let mut execution = execution_with_auth(
            RuntimeAuthRef::CustomCredential {
                auth_scheme: AuthScheme::Bearer,
                slots: vec![
                    "/settingsConfig/apiKey".to_string(),
                    "/settingsConfig/extraHeaders/X-Tenant".to_string(),
                ],
                credential_generation: 4,
            },
            UpstreamProtocol::OpenAiResponses,
            json!({
                "apiKey": "primary-secret",
                "extraHeaders": {"X-Tenant": "tenant-secret"}
            }),
            4,
        );
        execution.plan = Arc::new(ProviderRuntimePlan {
            extra_headers: vec![crate::domain::providers::runtime::RuntimeExtraHeaderRef {
                name: "x-tenant".to_string(),
                credential_slot: "/settingsConfig/extraHeaders/X-Tenant".to_string(),
            }],
            ..execution.plan.as_ref().clone()
        });

        let application = execution
            .materialize_auth(&AccountStore::default())
            .unwrap();
        let auth = application.injected_values().unwrap();

        assert_eq!(
            auth.headers,
            vec![
                (
                    "authorization".to_string(),
                    "Bearer primary-secret".to_string()
                ),
                ("x-tenant".to_string(), "tenant-secret".to_string()),
            ]
        );
    }
}
