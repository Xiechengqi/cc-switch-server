use std::sync::Arc;

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;
use zeroize::Zeroize;

use crate::domain::accounts::managers::{manager_for, AccountManager, CredentialKind};
use crate::domain::accounts::store::{effective_codex_workspace_id, Account, AccountStore};
use crate::domain::providers::registry::{
    provider_registry, AuthScheme, OperationSupport, UpstreamProtocol,
};
use crate::domain::providers::runtime::{
    ProviderRuntimePlan, RuntimeAuthRef, RuntimeConfigurationState, RuntimeModelPolicy,
};
use crate::domain::providers::store::{ProviderStore, StoredProvider};

use super::adapters::{self, AdapterRequest};
use super::router::ProxyRoute;
use super::{codex_provider_api_key, setting, ProxyError};

#[derive(Clone)]
pub(crate) struct ProviderExecution {
    pub stored: StoredProvider,
    pub plan: Arc<ProviderRuntimePlan>,
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
                "special.cursor" | "special.copilot" => Some("openai_chat"),
                "special.antigravity" | "special.agy" => Some("gemini_native"),
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
            RuntimeAuthRef::Legacy { account_id, .. } => account_id.as_deref(),
            _ => None,
        }
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

    pub fn materialize_auth(
        &self,
        accounts: &AccountStore,
    ) -> Result<Option<MaterializedAuth>, ProxyError> {
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
                Some(managed_auth(self, account, credential))
            }
            RuntimeAuthRef::StaticCredential {
                auth_scheme,
                credential_generation,
                ..
            } => {
                self.ensure_credential_generation(*credential_generation)?;
                let secret = provider_secret(&self.stored).ok_or_else(|| {
                    ProxyError::bad_request("Provider credential is not configured")
                })?;
                Some(static_auth(self, *auth_scheme, secret)?)
            }
            RuntimeAuthRef::CustomCredential {
                auth_scheme,
                credential_generation,
                ..
            } => {
                self.ensure_credential_generation(*credential_generation)?;
                if *auth_scheme == AuthScheme::None {
                    Some(MaterializedAuth::default())
                } else {
                    let secret = provider_secret(&self.stored).ok_or_else(|| {
                        ProxyError::bad_request("custom Provider credential is not configured")
                    })?;
                    Some(static_auth(self, *auth_scheme, secret)?)
                }
            }
            RuntimeAuthRef::AwsCredential {
                credential_generation,
                ..
            } => {
                self.ensure_credential_generation(*credential_generation)?;
                Some(MaterializedAuth::default())
            }
            RuntimeAuthRef::Legacy { .. } => None,
            RuntimeAuthRef::Missing => {
                return Err(ProxyError::bad_request(format!(
                    "Provider {} credential binding is incomplete",
                    self.stored.provider.id
                )))
            }
        };
        if let Some(auth) = materialized.as_mut() {
            self.append_extra_headers(auth)?;
        }
        Ok(materialized)
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
        auth: Option<&MaterializedAuth>,
    ) -> Result<(), ProxyError> {
        let Some(auth) = auth else {
            return Ok(());
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
            {
                let mut query = parsed.query_pairs_mut();
                for (name, value) in &auth.query {
                    query.append_pair(name, value);
                }
            }
            *url = parsed.to_string();
        }
        Ok(())
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
                if request.pricing_model.is_none() {
                    request.pricing_model = request.actual_model.clone();
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
                request.pricing_model = Some(upstream_model.to_string());
            }
        }
        Ok(())
    }

    pub fn finalize_request(&self, request: &mut AdapterRequest) -> Result<(), ProxyError> {
        adapters::finalize_runtime_request(&self.plan, &self.stored, request)
    }

    pub fn resolve_endpoint(
        &self,
        route: ProxyRoute,
        gemini_path: Option<String>,
        request: &AdapterRequest,
    ) -> Result<String, ProxyError> {
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
                matches!(self.plan.auth_ref, RuntimeAuthRef::ManagedAccount { .. }),
            )?;
            request.model = Some(contract.actual_model.clone());
            request.actual_model = Some(contract.actual_model.clone());
            request.actual_model_source = Some("grok_model_normalization".to_string());
            request.pricing_model = Some(contract.actual_model);
            for (name, value) in contract.headers {
                replace_header(headers, name, &value);
            }
            *endpoint = super::grok::chat_upstream_url(
                endpoint,
                matches!(self.plan.auth_ref, RuntimeAuthRef::ManagedAccount { .. }),
            );
            self.enforce_model_policy(request)?;
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
) -> MaterializedAuth {
    if matches!(
        execution.plan.driver_id.as_str(),
        "special.cursor" | "special.kiro" | "special.deepseek_account" | "special.copilot"
    ) {
        return MaterializedAuth::default();
    }
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
    auth
}

fn static_auth(
    execution: &ProviderExecution,
    scheme: AuthScheme,
    secret: String,
) -> Result<MaterializedAuth, ProxyError> {
    let mut auth = MaterializedAuth::default();
    match scheme {
        AuthScheme::None => {}
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
    Ok(auth)
}

fn validate_custom_auth_header(name: &str) -> Result<(), ProxyError> {
    crate::domain::providers::runtime::validate_custom_header_name(name)
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
            "special.cursor" | "special.copilot" | "special.kiro" | "special.deepseek_account"
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
                auth_ref,
                model_policy: RuntimeModelPolicy::Passthrough,
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
                auth_ref: RuntimeAuthRef::Missing,
                model_policy: RuntimeModelPolicy::Single {
                    upstream_model: "actual-model".to_string(),
                },
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
            pricing_model: None,
            stream_requested: false,
            custom_tool_names: Default::default(),
        };

        execution.enforce_model_policy(&mut request).unwrap();

        assert_eq!(request.requested_model.as_deref(), Some("requested-model"));
        assert_eq!(request.actual_model.as_deref(), Some("actual-model"));
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["model"], "actual-model");
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
        let api_key_auth = api_key
            .materialize_auth(&AccountStore::default())
            .unwrap()
            .unwrap();
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
        let bearer_auth = bearer
            .materialize_auth(&AccountStore::default())
            .unwrap()
            .unwrap();
        assert_eq!(
            bearer_auth.headers,
            vec![("authorization".to_string(), "Bearer secret-key".to_string())]
        );
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
            })
            .unwrap_err();
        assert_eq!(error.status, StatusCode::CONFLICT);
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

        let auth = execution
            .materialize_auth(&AccountStore::default())
            .unwrap()
            .unwrap();

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
