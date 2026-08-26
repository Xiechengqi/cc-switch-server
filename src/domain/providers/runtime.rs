use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{bail, Context};
use http::HeaderName;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use url::Url;

use crate::domain::accounts::managers::{account_credential_ownership, AccountCredentialOwnership};
use crate::domain::accounts::store::{Account, AccountStore};

use super::coding_plan::{compile_profile_contract, RuntimeCodingPlan};
use super::credentials::redact_provider;
use super::model::{AppKind, Provider, ProviderType};
use super::model_routing::{policy_from_settings, ModelRoutingMode};
use super::registry::{
    profile_by_id, provider_registry, resolve_custom_binding, AuthScheme, CredentialPolicy,
    DriverBinding, DriverId, EndpointPolicy, ModelPolicyKind, OutboundIdentityPolicy, ProfileId,
    ProfileSpec, UpstreamProtocol,
};
use super::store::{ProviderStore, StoredProvider};

pub const MIN_PROVIDER_TIMEOUT_SECONDS: u64 = 1;
pub const MAX_PROVIDER_REQUEST_TIMEOUT_SECONDS: u64 = 3_600;
pub const MAX_PROVIDER_FIRST_BYTE_TIMEOUT_SECONDS: u64 = 600;
pub const MAX_PROVIDER_IDLE_TIMEOUT_SECONDS: u64 = 3_600;
pub const PROVIDER_MODEL_PROBE_PROMPT: &str = "ping";
pub const PROVIDER_MODEL_PROBE_PAYLOAD_REVISION: u32 = 2;

/// Public, credential-free description of the exact model request used to
/// probe a Provider runtime.  The Router renders connection examples from this
/// value and asks the Server to execute the same probe; neither consumer needs
/// to infer Provider-specific model or payload rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderModelProbe {
    /// Public API vocabulary.  This deliberately differs from the internal
    /// product names (`codex` / `claude`) used by AppKind.
    pub api_type: String,
    /// Authoritative Server test model, including optional modifiers such as
    /// Codex `@low`.
    pub requested_model: String,
    /// Model value placed on the public API wire.  For example
    /// `gpt-5.6-luna@low` becomes `gpt-5.6-luna` plus a reasoning field.
    pub wire_model: String,
    pub method: String,
    pub path: String,
    pub body: Value,
    pub stream: bool,
    pub response_mode: String,
    pub payload_revision: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub health_fingerprint: String,
}

impl ProviderModelProbe {
    pub fn body_json(&self) -> String {
        serde_json::to_string(&self.body).unwrap_or_else(|_| "{}".to_string())
    }
}

pub fn build_provider_model_probe(
    app: AppKind,
    provider_type: ProviderType,
    requested_model: &str,
    prompt: &str,
    stream: bool,
    health_fingerprint: impl Into<String>,
) -> ProviderModelProbe {
    let requested_model = requested_model.trim().to_string();
    let (wire_model, reasoning_effort) = if app == AppKind::Codex {
        split_probe_model_modifier(&requested_model)
    } else {
        (requested_model.clone(), None)
    };
    let (api_type, path, body, response_mode) = match app {
        AppKind::Claude => (
            "anthropic",
            "/v1/messages".to_string(),
            json!({
                "model": wire_model.clone(),
                "max_tokens": 1,
                "messages": [{"role": "user", "content": prompt}],
                "stream": stream,
            }),
            if stream { "anthropic_sse" } else { "json" },
        ),
        AppKind::Codex => {
            let mut body = json!({
                "model": wire_model.clone(),
                "input": [{"role": "user", "content": prompt}],
                "stream": stream,
            });
            if let Some(effort) = reasoning_effort {
                body["reasoning"] = json!({ "effort": effort });
            } else if provider_type == ProviderType::CodexOAuth {
                body["reasoning"] = json!({ "effort": "low" });
            } else {
                body["max_output_tokens"] = json!(1);
            }
            if provider_type == ProviderType::CodexOAuth {
                body["store"] = json!(false);
                body["include"] = json!(["reasoning.encrypted_content"]);
                body["instructions"] = json!("");
                body["tools"] = json!([]);
                body["parallel_tool_calls"] = json!(false);
            }
            (
                "openai",
                "/v1/responses".to_string(),
                body,
                if stream { "responses_sse" } else { "json" },
            )
        }
        AppKind::Gemini => {
            let operation = if stream {
                "streamGenerateContent?alt=sse"
            } else {
                "generateContent"
            };
            (
                "gemini",
                format!(
                    "/v1beta/models/{}:{operation}",
                    encode_probe_path_segment(&wire_model)
                ),
                json!({
                    "contents": [{"role": "user", "parts": [{"text": prompt}]}],
                    "generationConfig": {"maxOutputTokens": 1},
                }),
                if stream { "gemini_sse" } else { "json" },
            )
        }
    };
    ProviderModelProbe {
        api_type: api_type.to_string(),
        requested_model,
        wire_model,
        method: "POST".to_string(),
        path,
        body,
        stream,
        response_mode: response_mode.to_string(),
        payload_revision: PROVIDER_MODEL_PROBE_PAYLOAD_REVISION,
        health_fingerprint: health_fingerprint.into(),
    }
}

fn split_probe_model_modifier(model: &str) -> (String, Option<String>) {
    if let Some(position) = model.find('@').or_else(|| model.find('#')) {
        let wire_model = model[..position].trim();
        let modifier = model[position + 1..].trim();
        if !wire_model.is_empty() && !modifier.is_empty() {
            return (wire_model.to_string(), Some(modifier.to_string()));
        }
    }
    (model.trim().to_string(), None)
}

fn encode_probe_path_segment(value: &str) -> String {
    let mut url =
        url::Url::parse("https://probe.invalid/v1beta/models").expect("static probe URL is valid");
    url.path_segments_mut()
        .expect("static probe URL can accept path segments")
        .push(value);
    url.path()
        .strip_prefix("/v1beta/models/")
        .unwrap_or(value)
        .to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderRequestDefaults {
    pub request_timeout_seconds: u64,
    pub stream_first_byte_timeout_seconds: u64,
    pub stream_idle_timeout_seconds: u64,
}

impl Default for ProviderRequestDefaults {
    fn default() -> Self {
        Self {
            request_timeout_seconds: 300,
            stream_first_byte_timeout_seconds: 120,
            stream_idle_timeout_seconds: 300,
        }
    }
}

impl ProviderRequestDefaults {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_timeout_range(
            "request timeout",
            self.request_timeout_seconds,
            MAX_PROVIDER_REQUEST_TIMEOUT_SECONDS,
        )?;
        validate_timeout_range(
            "stream first-byte timeout",
            self.stream_first_byte_timeout_seconds,
            MAX_PROVIDER_FIRST_BYTE_TIMEOUT_SECONDS,
        )?;
        validate_timeout_range(
            "stream idle timeout",
            self.stream_idle_timeout_seconds,
            MAX_PROVIDER_IDLE_TIMEOUT_SECONDS,
        )
    }

    pub fn transport_defaults(&self) -> ProviderTransportDefaults {
        ProviderTransportDefaults {
            timeout_ms: self.request_timeout_seconds.saturating_mul(1_000),
            stream_first_byte_timeout_ms: self
                .stream_first_byte_timeout_seconds
                .saturating_mul(1_000),
            stream_idle_timeout_ms: self.stream_idle_timeout_seconds.saturating_mul(1_000),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderTransportDefaults {
    pub timeout_ms: u64,
    pub stream_first_byte_timeout_ms: u64,
    pub stream_idle_timeout_ms: u64,
}

impl Default for ProviderTransportDefaults {
    fn default() -> Self {
        ProviderRequestDefaults::default().transport_defaults()
    }
}

impl ProviderTransportDefaults {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_timeout_range_ms("request timeout", self.timeout_ms, 3_600_000)?;
        validate_timeout_range_ms(
            "stream first-byte timeout",
            self.stream_first_byte_timeout_ms,
            600_000,
        )?;
        validate_timeout_range_ms(
            "stream idle timeout",
            self.stream_idle_timeout_ms,
            3_600_000,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderTestModelDefaults {
    pub claude: String,
    pub codex: String,
    pub gemini: String,
}

impl Default for ProviderTestModelDefaults {
    fn default() -> Self {
        Self {
            claude: "claude-haiku-4-5-20251001".to_string(),
            codex: "gpt-5.6-luna@low".to_string(),
            gemini: "gemini-3.5-flash".to_string(),
        }
    }
}

impl ProviderTestModelDefaults {
    pub fn for_app(&self, app: AppKind) -> &str {
        match app {
            AppKind::Claude => &self.claude,
            AppKind::Codex => &self.codex,
            AppKind::Gemini => &self.gemini,
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        for (app, model) in [
            (AppKind::Claude, self.claude.as_str()),
            (AppKind::Codex, self.codex.as_str()),
            (AppKind::Gemini, self.gemini.as_str()),
        ] {
            if model.trim().is_empty() || model != model.trim() || model.len() > 256 {
                bail!(
                    "default test model for {} must be non-empty, trimmed, and at most 256 characters",
                    app.as_str()
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderHealthCheckConfig {
    pub timeout_seconds: u64,
    pub max_retries: u32,
    pub degraded_threshold_seconds: u64,
    pub test_models: ProviderTestModelDefaults,
}

impl Default for ProviderHealthCheckConfig {
    fn default() -> Self {
        Self {
            timeout_seconds: 45,
            max_retries: 2,
            degraded_threshold_seconds: 6,
            test_models: ProviderTestModelDefaults::default(),
        }
    }
}

impl ProviderHealthCheckConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            (2..=60).contains(&self.timeout_seconds),
            "Provider health-check timeoutSeconds must be between 2 and 60"
        );
        anyhow::ensure!(
            self.max_retries <= 5,
            "Provider health-check maxRetries must be between 0 and 5"
        );
        anyhow::ensure!(
            (1..=30).contains(&self.degraded_threshold_seconds),
            "Provider health-check degradedThresholdSeconds must be between 1 and 30"
        );
        self.test_models.validate()
    }

    pub fn degraded_threshold_ms(&self) -> u64 {
        self.degraded_threshold_seconds.saturating_mul(1_000)
    }

    fn probe_policy_fingerprint(&self) -> String {
        runtime_fingerprint(&json!({
            "payloadRevision": PROVIDER_MODEL_PROBE_PAYLOAD_REVISION,
            "timeoutSeconds": self.timeout_seconds,
            "maxRetries": self.max_retries,
            "degradedThresholdSeconds": self.degraded_threshold_seconds,
        }))
        .expect("Provider probe policy fingerprint input is serializable")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRuntimeDefaults {
    pub transport: ProviderTransportDefaults,
    pub test_models: ProviderTestModelDefaults,
    pub probe_policy_fingerprint: String,
}

impl Default for ProviderRuntimeDefaults {
    fn default() -> Self {
        Self::from_settings(
            &ProviderRequestDefaults::default(),
            &ProviderHealthCheckConfig::default(),
        )
    }
}

impl ProviderRuntimeDefaults {
    pub fn from_settings(
        request: &ProviderRequestDefaults,
        health: &ProviderHealthCheckConfig,
    ) -> Self {
        Self {
            transport: request.transport_defaults(),
            test_models: health.test_models.clone(),
            probe_policy_fingerprint: health.probe_policy_fingerprint(),
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        self.transport.validate()?;
        self.test_models.validate()
    }
}

fn validate_timeout_range(label: &str, value: u64, max: u64) -> anyhow::Result<()> {
    if !(MIN_PROVIDER_TIMEOUT_SECONDS..=max).contains(&value) {
        bail!("{label} must be between {MIN_PROVIDER_TIMEOUT_SECONDS} and {max} seconds");
    }
    Ok(())
}

fn validate_timeout_range_ms(label: &str, value: u64, max: u64) -> anyhow::Result<()> {
    if !(1_000..=max).contains(&value) {
        bail!("{label} must be between 1000 and {max} milliseconds");
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeConfigurationState {
    Ready,
    LegacyCompat,
    NeedsAttention,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum RuntimeModelPolicy {
    Passthrough,
    Single { upstream_model: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum RuntimeAuthRef {
    ManagedAccount {
        account_id: String,
        expected_provider_type: ProviderType,
        auth_identity_generation: u64,
    },
    StaticCredential {
        auth_scheme: AuthScheme,
        slots: Vec<String>,
        credential_generation: u64,
    },
    AwsCredential {
        slots: Vec<String>,
        credential_generation: u64,
    },
    CustomCredential {
        auth_scheme: AuthScheme,
        slots: Vec<String>,
        credential_generation: u64,
    },
    Legacy {
        account_id: Option<String>,
        credential_generation: u64,
    },
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeExtraHeaderRef {
    pub name: String,
    pub credential_slot: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTransportPolicy {
    pub timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_first_byte_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_idle_timeout_ms: Option<u64>,
    pub redirect_policy: String,
    pub direct_connection: bool,
}

impl Default for RuntimeTransportPolicy {
    fn default() -> Self {
        let defaults = ProviderTransportDefaults::default();
        Self {
            timeout_ms: defaults.timeout_ms,
            stream_first_byte_timeout_ms: Some(defaults.stream_first_byte_timeout_ms),
            stream_idle_timeout_ms: Some(defaults.stream_idle_timeout_ms),
            redirect_policy: "same_origin".to_string(),
            direct_connection: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRuntimePlan {
    pub provider_key: super::registry::ProviderKey,
    pub provider_revision: u64,
    pub profile_id: ProfileId,
    pub profile_schema_revision: u32,
    pub driver_id: DriverId,
    pub driver_contract_revision: u32,
    pub endpoint: String,
    pub upstream_protocol: UpstreamProtocol,
    pub outbound_identity_policy: OutboundIdentityPolicy,
    pub auth_ref: RuntimeAuthRef,
    pub model_policy: RuntimeModelPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coding_plan: Option<RuntimeCodingPlan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_model: Option<String>,
    pub probe_policy_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aws_region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_policy: Option<Value>,
    pub transport_policy: RuntimeTransportPolicy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_headers: Vec<RuntimeExtraHeaderRef>,
    #[serde(default)]
    pub driver_options: BTreeMap<String, Value>,
    pub configuration_state: RuntimeConfigurationState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    pub runtime_fingerprint: String,
}

impl ProviderRuntimePlan {
    pub fn health_fingerprint(&self) -> String {
        runtime_fingerprint(&json!({
            "runtimeFingerprint": self.runtime_fingerprint,
            "testModel": self.test_model,
            "probePolicyFingerprint": self.probe_policy_fingerprint,
        }))
        .expect("Provider health fingerprint input is serializable")
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProviderRuntimeIndex {
    plans: BTreeMap<super::registry::ProviderKey, Arc<ProviderRuntimePlan>>,
}

impl ProviderRuntimeIndex {
    pub fn compile(store: &ProviderStore, accounts: &AccountStore) -> anyhow::Result<Self> {
        let mut plans = BTreeMap::new();
        for stored in &store.providers {
            if !super::bundle::surface_enabled(&stored.provider) {
                continue;
            }
            let plan = Arc::new(compile_runtime_plan_with_defaults(
                stored,
                accounts,
                store.runtime_defaults(),
            )?);
            if plans.insert(plan.provider_key.clone(), plan).is_some() {
                bail!("duplicate Provider key while compiling runtime index");
            }
        }
        Ok(Self { plans })
    }

    pub fn get(&self, app: AppKind, provider_id: &str) -> Option<Arc<ProviderRuntimePlan>> {
        let key = super::registry::ProviderKey::new(app, provider_id).ok()?;
        self.plans.get(&key).cloned()
    }

    pub fn len(&self) -> usize {
        self.plans.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plans.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn insert_plan_for_test(&mut self, plan: ProviderRuntimePlan) {
        self.plans.insert(plan.provider_key.clone(), Arc::new(plan));
    }
}

pub fn compile_runtime_plan(
    stored: &StoredProvider,
    accounts: &AccountStore,
) -> anyhow::Result<ProviderRuntimePlan> {
    compile_runtime_plan_with_defaults(stored, accounts, &ProviderRuntimeDefaults::default())
}

pub fn compile_runtime_plan_with_defaults(
    stored: &StoredProvider,
    accounts: &AccountStore,
    defaults: &ProviderRuntimeDefaults,
) -> anyhow::Result<ProviderRuntimePlan> {
    let mut warnings = Vec::new();
    let mut configuration_state = RuntimeConfigurationState::Ready;
    let (profile_id, profile_schema_revision, driver_id, profile_policy) = if let Some(profile_id) =
        stored.resource.profile_id.as_ref()
    {
        let profile = profile_by_id(profile_id.as_str())
            .with_context(|| format!("Provider {} has an unknown profileId", stored.provider.id))?;
        let driver_id = match &profile.driver_binding {
            DriverBinding::Fixed { driver_id } => driver_id.clone(),
            DriverBinding::Custom { .. } => match stored.resource.custom_binding.as_ref() {
                Some(binding) => resolve_custom_binding(profile, binding)?.driver_id,
                None => {
                    configuration_state = RuntimeConfigurationState::NeedsAttention;
                    warnings
                        .push("custom Provider has no explicit protocol/auth binding".to_string());
                    legacy_driver_id(stored)?
                }
            },
        };
        (
            profile.profile_id.clone(),
            stored
                .resource
                .profile_schema_revision
                .unwrap_or(profile.profile_schema_revision),
            driver_id,
            Some(profile),
        )
    } else {
        configuration_state = RuntimeConfigurationState::LegacyCompat;
        warnings.push("legacy Provider is running with a frozen compatibility plan".to_string());
        (
            legacy_profile_id(stored.app)?,
            1,
            legacy_driver_id(stored)?,
            None,
        )
    };

    let driver = provider_registry()
        .drivers
        .iter()
        .find(|driver| driver.driver_id == driver_id)
        .with_context(|| format!("runtime Driver {driver_id} is not registered"))?;
    let coding_plan = profile_policy
        .and_then(|profile| profile.coding_plan.as_ref())
        .map(|contract| compile_profile_contract(stored.app, contract))
        .transpose()?;
    let configured_endpoint = configured_base_url(&stored.provider, stored.app);
    let default_endpoint = default_base_url(stored.provider_type).map(str::to_string);
    let endpoint_policy = profile_policy
        .map(|profile| profile.endpoint_policy)
        .unwrap_or_else(|| {
            if managed_oauth_endpoint_is_fixed(stored.provider_type) {
                EndpointPolicy::Fixed
            } else {
                EndpointPolicy::FrozenLegacy
            }
        });
    let endpoint = if let Some(contract) = coding_plan.as_ref() {
        if configured_endpoint
            .as_deref()
            .is_some_and(|configured| !endpoints_equivalent(configured, &contract.fixed_origin))
        {
            warnings
                .push("coding-plan contract ignored a configured endpoint override".to_string());
        }
        Some(contract.fixed_origin.clone())
    } else {
        match endpoint_policy {
            EndpointPolicy::Fixed => {
                if configured_endpoint.as_deref().is_some_and(|configured| {
                    default_endpoint
                        .as_deref()
                        .is_none_or(|default| !endpoints_equivalent(configured, default))
                }) {
                    warnings.push(
                        "fixed endpoint policy ignored a configured endpoint override".to_string(),
                    );
                }
                default_endpoint
            }
            EndpointPolicy::OverrideAllowed
            | EndpointPolicy::Template
            | EndpointPolicy::FrozenLegacy => configured_endpoint.or(default_endpoint),
            EndpointPolicy::Custom => configured_endpoint,
        }
    };
    #[cfg(test)]
    let endpoint = if coding_plan.is_some() {
        endpoint
    } else {
        configured_setting(&stored.provider, "testRuntimeEndpoint").or(endpoint)
    };
    let endpoint = match endpoint {
        Some(endpoint) => match validate_endpoint(&endpoint, stored) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                configuration_state = RuntimeConfigurationState::NeedsAttention;
                warnings.push(error.to_string());
                endpoint.trim().to_string()
            }
        },
        None => {
            configuration_state = RuntimeConfigurationState::NeedsAttention;
            warnings.push("Provider endpoint is not configured".to_string());
            String::new()
        }
    };

    let model_policy = runtime_model_policy(stored, profile_policy);
    let model_policy = match model_policy {
        Ok(policy) => policy,
        Err(error) => {
            configuration_state = RuntimeConfigurationState::NeedsAttention;
            warnings.push(error.to_string());
            RuntimeModelPolicy::Single {
                upstream_model: String::new(),
            }
        }
    };
    let auth_ref = runtime_auth_ref(stored, accounts, profile_policy, &mut warnings);
    if matches!(auth_ref, RuntimeAuthRef::Missing) {
        configuration_state = RuntimeConfigurationState::NeedsAttention;
    }
    let outbound_identity_policy = runtime_outbound_identity_policy(profile_policy, driver)?;
    let driver_options = match runtime_driver_options(&stored.provider, outbound_identity_policy) {
        Ok(options) => options,
        Err(error) => {
            configuration_state = RuntimeConfigurationState::NeedsAttention;
            warnings.push(error.to_string());
            BTreeMap::new()
        }
    };
    let test_model = runtime_test_model(stored, profile_policy.is_some(), defaults);
    let aws_region = configured_setting(&stored.provider, "AWS_REGION");
    let media_policy = runtime_media_policy(&stored.provider, profile_policy.is_none());
    let transport_policy = runtime_transport_policy(
        &stored.provider,
        profile_policy.is_some(),
        &defaults.transport,
    );
    let extra_headers = match runtime_extra_headers(stored, profile_policy) {
        Ok(headers) => headers,
        Err(error) => {
            configuration_state = RuntimeConfigurationState::NeedsAttention;
            warnings.push(error.to_string());
            Vec::new()
        }
    };
    let provider_key = super::registry::ProviderKey::new(stored.app, &stored.provider.id)?;
    let runtime_fingerprint = runtime_fingerprint(&json!({
        "providerKey": &provider_key,
        "profileId": &profile_id,
        "profileSchemaRevision": profile_schema_revision,
        "driverId": &driver_id,
        "driverContractRevision": driver.driver_contract_revision,
        "endpoint": &endpoint,
        "upstreamProtocol": driver.upstream_protocol,
        "outboundIdentityPolicy": outbound_identity_policy,
        "authRef": &auth_ref,
        "cursorVerifiedIdentity": &stored.resource.cursor_verified_identity,
        "modelPolicy": &model_policy,
        "codingPlan": &coding_plan,
        "awsRegion": &aws_region,
        "mediaPolicy": &media_policy,
        "transportPolicy": &transport_policy,
        "extraHeaders": &extra_headers,
        "driverOptions": &driver_options,
    }))?;

    Ok(ProviderRuntimePlan {
        provider_key,
        provider_revision: stored.resource.revision,
        profile_id,
        profile_schema_revision,
        driver_id,
        driver_contract_revision: driver.driver_contract_revision,
        endpoint,
        upstream_protocol: driver.upstream_protocol,
        outbound_identity_policy,
        auth_ref,
        model_policy,
        coding_plan,
        test_model,
        probe_policy_fingerprint: defaults.probe_policy_fingerprint.clone(),
        aws_region,
        media_policy,
        transport_policy,
        extra_headers,
        driver_options,
        configuration_state,
        warnings,
        runtime_fingerprint,
    })
}

pub fn validate_custom_extra_headers(
    stored: &StoredProvider,
    profile: &super::registry::ProfileSpec,
) -> anyhow::Result<()> {
    runtime_extra_headers(stored, Some(profile)).map(|_| ())
}

fn runtime_extra_headers(
    stored: &StoredProvider,
    profile: Option<&super::registry::ProfileSpec>,
) -> anyhow::Result<Vec<RuntimeExtraHeaderRef>> {
    let Some(raw) = stored.provider.settings_config.get("extraHeaders") else {
        return Ok(Vec::new());
    };
    let Some(headers) = raw.as_object() else {
        bail!("custom extraHeaders must be an object of header names to secret values");
    };
    if headers.is_empty() {
        return Ok(Vec::new());
    }
    if !profile.is_some_and(|profile| matches!(profile.credential_policy, CredentialPolicy::Custom))
    {
        bail!("extraHeaders are only supported by a custom Provider profile");
    }
    if headers.len() > 32 {
        bail!("custom extraHeaders cannot contain more than 32 headers");
    }

    let mut seen = std::collections::BTreeSet::new();
    let mut refs = Vec::with_capacity(headers.len());
    for (stored_name, value) in headers {
        let name = validate_custom_header_name(stored_name)?;
        if !seen.insert(name.clone()) {
            bail!("custom extraHeaders contain a duplicate header name: {name}");
        }
        if value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
        {
            bail!("custom extra header {name} must have a non-empty secret string value");
        }
        refs.push(RuntimeExtraHeaderRef {
            credential_slot: format!(
                "/settingsConfig/extraHeaders/{}",
                escape_json_pointer_segment(stored_name)
            ),
            name,
        });
    }
    refs.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(refs)
}

pub fn validate_custom_header_name(name: &str) -> anyhow::Result<String> {
    let canonical = parse_custom_header_name(name)?;
    if matches!(
        canonical.as_str(),
        "authorization" | "x-api-key" | "api-key" | "x-goog-api-key"
    ) {
        bail!("custom header {canonical} is controlled by the Provider driver");
    }
    validate_custom_auth_header_name(&canonical)
}

pub fn validate_custom_auth_header_name(name: &str) -> anyhow::Result<String> {
    let canonical = parse_custom_header_name(name)?;
    if matches!(
        canonical.as_str(),
        "proxy-authorization"
            | "proxy-authenticate"
            | "host"
            | "content-length"
            | "content-type"
            | "connection"
            | "keep-alive"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "user-agent"
    ) {
        bail!("custom authentication header {canonical} is controlled by the Provider driver");
    }
    Ok(canonical)
}

fn parse_custom_header_name(name: &str) -> anyhow::Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("custom header name cannot be empty");
    }
    let parsed = HeaderName::from_bytes(name.as_bytes())
        .with_context(|| format!("custom header name is invalid: {name}"))?;
    let canonical = parsed.as_str().to_string();
    Ok(canonical)
}

fn escape_json_pointer_segment(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn runtime_auth_ref(
    stored: &StoredProvider,
    accounts: &AccountStore,
    profile: Option<&super::registry::ProfileSpec>,
    warnings: &mut Vec<String>,
) -> RuntimeAuthRef {
    let Some(profile) = profile else {
        if account_credential_ownership(stored.provider_type)
            == AccountCredentialOwnership::ManagedAccount
        {
            if provider_account_id(stored).is_some() {
                return managed_account_auth_ref(stored, accounts, stored.provider_type, warnings);
            }
            warnings.push(format!(
                "{} Provider must explicitly bind a managed {} account",
                stored.provider_type.as_str(),
                stored.provider_type.as_str()
            ));
            return RuntimeAuthRef::Missing;
        }
        return RuntimeAuthRef::Legacy {
            account_id: None,
            credential_generation: stored.resource.credential_generation,
        };
    };
    match &profile.credential_policy {
        CredentialPolicy::ManagedAccount {
            account_provider_type,
        } => managed_account_auth_ref(stored, accounts, *account_provider_type, warnings),
        CredentialPolicy::StaticSecret { slots, auth_scheme } => {
            let summary = redact_provider(&stored.provider).1;
            let (slots, auth_scheme, credential_configured) =
                if let Some(contract) = profile.coding_plan.as_ref() {
                    (
                        vec![contract.inference.credential_slot.clone()],
                        contract.inference.auth_scheme,
                        summary
                            .slots
                            .iter()
                            .any(|configured| configured == &contract.inference.credential_slot),
                    )
                } else {
                    (
                        normalized_slots(slots, &summary.slots),
                        *auth_scheme,
                        summary.configured,
                    )
                };
            if !credential_configured {
                warnings.push("Provider credential is not configured".to_string());
                return RuntimeAuthRef::Missing;
            }
            RuntimeAuthRef::StaticCredential {
                auth_scheme,
                slots,
                credential_generation: stored.resource.credential_generation,
            }
        }
        CredentialPolicy::Aws { slots } => {
            let summary = redact_provider(&stored.provider).1;
            let access_key_configured =
                configured_setting(&stored.provider, "AWS_ACCESS_KEY_ID").is_some();
            let secret_key_configured =
                configured_setting(&stored.provider, "AWS_SECRET_ACCESS_KEY").is_some();
            if !access_key_configured || !secret_key_configured {
                warnings.push(
                    "AWS Provider requires both AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY"
                        .to_string(),
                );
                return RuntimeAuthRef::Missing;
            }
            RuntimeAuthRef::AwsCredential {
                slots: normalized_slots(slots, &summary.slots),
                credential_generation: stored.resource.credential_generation,
            }
        }
        CredentialPolicy::Custom => {
            let Some(binding) = stored.resource.custom_binding.as_ref() else {
                return RuntimeAuthRef::Missing;
            };
            let summary = redact_provider(&stored.provider).1;
            let primary_credential_configured = summary
                .slots
                .iter()
                .any(|slot| !slot.starts_with("/settingsConfig/extraHeaders/"));
            if binding.auth_scheme != AuthScheme::None && !primary_credential_configured {
                warnings.push("custom Provider credential is not configured".to_string());
                return RuntimeAuthRef::Missing;
            }
            RuntimeAuthRef::CustomCredential {
                auth_scheme: binding.auth_scheme,
                slots: summary.slots,
                credential_generation: stored.resource.credential_generation,
            }
        }
        CredentialPolicy::Legacy => RuntimeAuthRef::Legacy {
            account_id: provider_account_id(stored).map(str::to_string),
            credential_generation: stored.resource.credential_generation,
        },
    }
}

fn managed_account_auth_ref(
    stored: &StoredProvider,
    accounts: &AccountStore,
    account_provider_type: ProviderType,
    warnings: &mut Vec<String>,
) -> RuntimeAuthRef {
    let Some(account_id) = provider_account_id(stored) else {
        warnings.push("managed Provider has no fixed accountId".to_string());
        return RuntimeAuthRef::Missing;
    };
    let Some(expected_generation) = provider_auth_identity_generation(stored) else {
        warnings.push("managed Provider has no auth identity generation".to_string());
        return RuntimeAuthRef::Missing;
    };
    let Some(account) = accounts
        .accounts
        .iter()
        .find(|account| account.id == account_id)
    else {
        warnings.push(format!("bound account {account_id} does not exist"));
        return RuntimeAuthRef::Missing;
    };
    if account.provider_type != account_provider_type {
        warnings.push(format!(
            "bound account {account_id} has providerType {}, expected {}",
            account.provider_type.as_str(),
            account_provider_type.as_str()
        ));
        return RuntimeAuthRef::Missing;
    }
    if account.auth_identity_generation != expected_generation {
        warnings.push(format!(
            "bound account {account_id} identity generation is stale"
        ));
        return RuntimeAuthRef::Missing;
    }
    RuntimeAuthRef::ManagedAccount {
        account_id: account_id.to_string(),
        expected_provider_type: account_provider_type,
        auth_identity_generation: expected_generation,
    }
}

pub fn has_authoritative_subscription_oauth_binding(stored: &StoredProvider) -> bool {
    if provider_account_id(stored).is_none()
        || !matches!(
            stored.provider_type,
            ProviderType::ClaudeOAuth
                | ProviderType::CodexOAuth
                | ProviderType::GrokOAuth
                | ProviderType::KimiCode
        )
    {
        return false;
    }
    let Some(profile_id) = stored.resource.profile_id.as_ref() else {
        return true;
    };
    profile_by_id(profile_id.as_str()).is_some_and(|profile| {
        profile.app == stored.app
            && matches!(
                &profile.credential_policy,
                CredentialPolicy::ManagedAccount {
                    account_provider_type
                } if *account_provider_type == stored.provider_type
            )
    })
}

fn normalized_slots(declared: &[String], discovered: &[String]) -> Vec<String> {
    let mut slots = declared
        .iter()
        .chain(discovered)
        .map(|slot| slot.trim())
        .filter(|slot| !slot.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    slots.sort();
    slots.dedup();
    slots
}

fn runtime_model_policy(
    stored: &StoredProvider,
    profile: Option<&ProfileSpec>,
) -> anyhow::Result<RuntimeModelPolicy> {
    let configured = policy_from_settings(&stored.provider.settings_config);
    let configured_kind = configured.as_ref().map(|policy| match policy.mode {
        ModelRoutingMode::Passthrough => ModelPolicyKind::Passthrough,
        ModelRoutingMode::Single => ModelPolicyKind::Single,
    });
    let kind = match profile {
        Some(profile) => {
            let kind = configured_kind.unwrap_or(profile.model_policy);
            if !profile.allows_model_policy(kind) {
                anyhow::bail!(
                    "Provider profile {} does not allow modelMapping.mode={}",
                    profile.profile_id,
                    match kind {
                        ModelPolicyKind::Passthrough => "passthrough",
                        ModelPolicyKind::Single => "single",
                    }
                );
            }
            kind
        }
        None => configured_kind.unwrap_or(ModelPolicyKind::Passthrough),
    };
    match kind {
        ModelPolicyKind::Passthrough => Ok(RuntimeModelPolicy::Passthrough),
        ModelPolicyKind::Single => runtime_single_model(&stored.provider.settings_config)
            .or_else(|| profile.and_then(|profile| profile.default_upstream_model.clone()))
            .map(|upstream_model| RuntimeModelPolicy::Single { upstream_model })
            .context("single-model Provider has no upstream model"),
    }
}

fn runtime_single_model(settings: &Value) -> Option<String> {
    policy_from_settings(settings)
        .and_then(|policy| policy.upstream_model)
        .or_else(|| {
            [
                "/model",
                "/config/model",
                "/env/ANTHROPIC_MODEL",
                "/env/OPENAI_MODEL",
                "/env/CODEX_MODEL",
                "/env/GEMINI_MODEL",
                "/env/GOOGLE_GEMINI_MODEL",
            ]
            .into_iter()
            .find_map(|pointer| non_empty_value(settings.pointer(pointer)))
        })
        .or_else(|| {
            settings
                .get("config")
                .and_then(Value::as_str)
                .and_then(|value| value.parse::<toml::Value>().ok())
                .and_then(|value| {
                    value
                        .get("model")
                        .and_then(toml::Value::as_str)
                        .map(str::to_string)
                })
        })
}

fn runtime_driver_options(
    provider: &Provider,
    outbound_identity_policy: OutboundIdentityPolicy,
) -> anyhow::Result<BTreeMap<String, Value>> {
    let mut options = BTreeMap::new();
    let Some(meta) = provider.meta.as_ref() else {
        return Ok(options);
    };
    for (name, value) in [
        (
            "apiKeyField",
            meta.api_key_field.as_ref().map(|value| json!(value)),
        ),
        ("isFullUrl", meta.is_full_url.map(|value| json!(value))),
        (
            "codexFastMode",
            meta.codex_fast_mode.map(|value| json!(value)),
        ),
        (
            "codexImageGenerationEnabled",
            meta.codex_image_generation_enabled
                .map(|value| json!(value)),
        ),
        (
            "codexWebsocketEnabled",
            meta.codex_websocket_enabled.map(|value| json!(value)),
        ),
        (
            "codexResponsesKeepaliveIntervalMs",
            meta.codex_responses_keepalive_interval_ms
                .map(|value| json!(value)),
        ),
        (
            "codexRoutingHintEnabled",
            meta.codex_routing_hint_enabled.map(|value| json!(value)),
        ),
    ] {
        if let Some(value) = value {
            options.insert(name.to_string(), value);
        }
    }
    #[cfg(test)]
    for name in [
        "testGrokWebsocketUrl",
        "testGrokModelsUrl",
        "testKiroModelsUrl",
        "testCopilotModelsUrl",
        "testCopilotInferenceUrl",
    ] {
        if let Some(value) = provider
            .settings_config
            .get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            options.insert(name.to_string(), json!(value));
        }
    }
    if outbound_identity_policy == OutboundIdentityPolicy::CustomOverride {
        if let Some(user_agent) = meta
            .custom_user_agent
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            validate_custom_user_agent(user_agent)?;
            options.insert("customUserAgent".to_string(), json!(user_agent));
        }
    }
    Ok(options)
}

fn runtime_outbound_identity_policy(
    profile: Option<&super::registry::ProfileSpec>,
    driver: &super::registry::DriverSpec,
) -> anyhow::Result<OutboundIdentityPolicy> {
    let Some(profile) = profile else {
        return Ok(driver.outbound_identity_policy);
    };
    let DriverBinding::Custom { custom_policy_id } = &profile.driver_binding else {
        return Ok(driver.outbound_identity_policy);
    };
    provider_registry()
        .custom_policies
        .iter()
        .find(|policy| policy.custom_policy_id == *custom_policy_id)
        .map(|policy| policy.outbound_identity_policy)
        .with_context(|| format!("custom policy {custom_policy_id} is not registered"))
}

pub fn validate_custom_user_agent(value: &str) -> anyhow::Result<String> {
    let value = value.trim();
    if value.is_empty() {
        bail!("custom User-Agent cannot be empty");
    }
    http::HeaderValue::from_str(value).context("custom User-Agent is not a valid header value")?;
    Ok(value.to_string())
}

fn runtime_test_model(
    stored: &StoredProvider,
    profiled: bool,
    defaults: &ProviderRuntimeDefaults,
) -> Option<String> {
    let model = non_empty_value(stored.provider.settings_config.get("testModel"))
        .or_else(|| super::bundle::bundle_test_model_override(&stored.provider).map(str::to_string))
        .or_else(|| {
            (!profiled)
                .then(|| {
                    non_empty_value(
                        stored
                            .provider
                            .settings_config
                            .pointer("/testConfig/testModel"),
                    )
                    .or_else(|| {
                        non_empty_value(
                            stored.provider.settings_config.pointer("/testConfig/model"),
                        )
                    })
                    .or_else(|| {
                        stored
                            .provider
                            .meta
                            .as_ref()
                            .and_then(|meta| meta.test_config.as_ref())
                            .and_then(|value| {
                                non_empty_value(
                                    value.get("testModel").or_else(|| value.get("model")),
                                )
                            })
                    })
                })
                .flatten()
        })
        .unwrap_or_else(|| defaults.test_models.for_app(stored.app).to_string());
    if stored.app == AppKind::Codex && stored.provider_type == ProviderType::CodexOAuth {
        let model = model.trim();
        if model.contains('@') || model.contains('#') {
            Some(model.to_string())
        } else {
            Some(format!("{model}@low"))
        }
    } else {
        Some(model)
    }
}

fn runtime_media_policy(provider: &Provider, legacy: bool) -> Option<Value> {
    if !legacy {
        return None;
    }
    let image_model = non_empty_value(provider.settings_config.get("imageModel"));
    let video_model = non_empty_value(provider.settings_config.get("videoModel"));
    (image_model.is_some() || video_model.is_some()).then(|| {
        json!({
            "imageModel": image_model,
            "videoModel": video_model,
        })
    })
}

fn runtime_transport_policy(
    provider: &Provider,
    profiled: bool,
    defaults: &ProviderTransportDefaults,
) -> RuntimeTransportPolicy {
    if profiled {
        return RuntimeTransportPolicy {
            timeout_ms: typed_timeout_ms(provider, "/transport/timeoutMs")
                .unwrap_or(defaults.timeout_ms),
            stream_first_byte_timeout_ms: Some(
                typed_timeout_ms(provider, "/transport/streamFirstByteTimeoutMs")
                    .unwrap_or(defaults.stream_first_byte_timeout_ms),
            ),
            stream_idle_timeout_ms: Some(
                typed_timeout_ms(provider, "/transport/streamIdleTimeoutMs")
                    .unwrap_or(defaults.stream_idle_timeout_ms),
            ),
            ..RuntimeTransportPolicy::default()
        };
    }
    RuntimeTransportPolicy {
        timeout_ms: configured_timeout_ms(
            provider,
            &[
                "UPSTREAM_TIMEOUT_MS",
                "PROXY_TIMEOUT_MS",
                "REQUEST_TIMEOUT_MS",
            ],
            defaults.timeout_ms,
        )
        .unwrap_or(defaults.timeout_ms),
        stream_first_byte_timeout_ms: configured_timeout_ms(
            provider,
            &[
                "STREAM_FIRST_BYTE_TIMEOUT_MS",
                "UPSTREAM_STREAM_FIRST_BYTE_TIMEOUT_MS",
                "FIRST_BYTE_TIMEOUT_MS",
            ],
            defaults.stream_first_byte_timeout_ms,
        ),
        stream_idle_timeout_ms: configured_timeout_ms(
            provider,
            &[
                "STREAM_IDLE_TIMEOUT_MS",
                "UPSTREAM_STREAM_IDLE_TIMEOUT_MS",
                "IDLE_TIMEOUT_MS",
            ],
            defaults.stream_idle_timeout_ms,
        ),
        ..RuntimeTransportPolicy::default()
    }
}

fn typed_timeout_ms(provider: &Provider, pointer: &str) -> Option<u64> {
    provider
        .settings_config
        .pointer(pointer)
        .and_then(Value::as_u64)
}

fn configured_timeout_ms(provider: &Provider, keys: &[&str], default_ms: u64) -> Option<u64> {
    let value = keys
        .iter()
        .find_map(|key| configured_setting(provider, key))
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default_ms);
    (value > 0).then_some(value)
}

fn configured_base_url(provider: &Provider, app: AppKind) -> Option<String> {
    let keys: &[&str] = match app {
        AppKind::Claude => &["ANTHROPIC_BASE_URL", "BASE_URL"],
        AppKind::Codex => &["OPENAI_BASE_URL", "CODEX_BASE_URL", "BASE_URL", "base_url"],
        AppKind::Gemini => &["GOOGLE_GEMINI_BASE_URL", "GEMINI_BASE_URL", "BASE_URL"],
    };
    for key in keys {
        if let Some(value) =
            non_empty_value(provider.settings_config.pointer(&format!("/env/{key}")))
                .or_else(|| non_empty_value(provider.settings_config.get(*key)))
        {
            return Some(value);
        }
    }
    if app == AppKind::Codex {
        if let Some(value) = non_empty_value(provider.settings_config.pointer("/config/base_url")) {
            return Some(value);
        }
        if let Some(value) = provider
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<toml::Value>().ok())
            .and_then(|value| {
                value
                    .get("base_url")
                    .and_then(toml::Value::as_str)
                    .map(str::to_string)
            })
        {
            return Some(value);
        }
    }
    None
}

fn non_empty_value(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn default_base_url(provider_type: ProviderType) -> Option<&'static str> {
    match provider_type {
        ProviderType::Claude | ProviderType::ClaudeAuth | ProviderType::ClaudeOAuth => {
            Some("https://api.anthropic.com")
        }
        ProviderType::Codex => Some("https://api.openai.com"),
        ProviderType::CodexOAuth => Some("https://chatgpt.com/backend-api/codex"),
        ProviderType::Gemini => Some("https://generativelanguage.googleapis.com"),
        ProviderType::GeminiCli => Some("https://cloudcode-pa.googleapis.com"),
        ProviderType::AntigravityOAuth | ProviderType::AgyOAuth => {
            Some("https://daily-cloudcode-pa.googleapis.com")
        }
        ProviderType::OpenRouter => Some("https://openrouter.ai/api"),
        ProviderType::GitHubCopilot => Some("https://api.githubcopilot.com"),
        ProviderType::DeepSeekAccount => Some("https://chat.deepseek.com"),
        ProviderType::KiroOAuth => Some("https://q.us-east-1.amazonaws.com"),
        ProviderType::KimiCode => Some("https://api.kimi.com/coding/v1"),
        ProviderType::QoderCosy => Some("https://api1.qoder.sh"),
        ProviderType::CursorOAuth => Some("https://api2.cursor.sh"),
        ProviderType::CursorApiKey => Some("https://api.cursor.com"),
        ProviderType::OllamaCloud => Some("https://ollama.com"),
        ProviderType::AwsBedrock => Some("https://bedrock-runtime.${AWS_REGION}.amazonaws.com"),
        ProviderType::Nvidia => Some("https://integrate.api.nvidia.com"),
        ProviderType::DeepSeekApi => Some("https://api.deepseek.com"),
        ProviderType::GrokOAuth => Some("https://api.x.ai/v1"),
    }
}

fn managed_oauth_endpoint_is_fixed(provider_type: ProviderType) -> bool {
    matches!(
        provider_type,
        ProviderType::ClaudeOAuth
            | ProviderType::CodexOAuth
            | ProviderType::GrokOAuth
            | ProviderType::KimiCode
            | ProviderType::QoderCosy
            | ProviderType::CursorOAuth
            | ProviderType::CursorApiKey
    )
}

fn endpoints_equivalent(left: &str, right: &str) -> bool {
    left.trim().trim_end_matches('/') == right.trim().trim_end_matches('/')
}

fn validate_endpoint(endpoint: &str, stored: &StoredProvider) -> anyhow::Result<String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        bail!("Provider endpoint is empty");
    }
    let parsed_value = if endpoint.contains("${AWS_REGION}") {
        endpoint.replace(
            "${AWS_REGION}",
            configured_setting(&stored.provider, "AWS_REGION")
                .as_deref()
                .unwrap_or("us-east-1"),
        )
    } else {
        endpoint.to_string()
    };
    let parsed = Url::parse(&parsed_value).context("Provider endpoint is not a valid URL")?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("Provider endpoint scheme must be http or https");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("Provider endpoint must not contain userinfo");
    }
    if parsed.host_str().is_none() {
        bail!("Provider endpoint must contain a host");
    }
    Ok(parsed_value.trim_end_matches('/').to_string())
}

fn configured_setting(provider: &Provider, key: &str) -> Option<String> {
    non_empty_value(provider.settings_config.pointer(&format!("/env/{key}")))
        .or_else(|| non_empty_value(provider.settings_config.get(key)))
}

fn provider_account_id(stored: &StoredProvider) -> Option<&str> {
    stored
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.auth_binding.as_ref())
        .and_then(|binding| binding.account_id.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub(crate) fn managed_account_provider_type(stored: &StoredProvider) -> Option<ProviderType> {
    if let Some(profile_id) = stored.resource.profile_id.as_ref() {
        let profile = profile_by_id(profile_id.as_str())?;
        return match &profile.credential_policy {
            CredentialPolicy::ManagedAccount {
                account_provider_type,
            } => Some(*account_provider_type),
            CredentialPolicy::Legacy
                if account_credential_ownership(stored.provider_type)
                    == AccountCredentialOwnership::ManagedAccount =>
            {
                Some(stored.provider_type)
            }
            _ => None,
        };
    }

    (account_credential_ownership(stored.provider_type)
        == AccountCredentialOwnership::ManagedAccount)
        .then_some(stored.provider_type)
}

pub(crate) fn managed_account_binding(stored: &StoredProvider) -> Option<(ProviderType, &str)> {
    Some((
        managed_account_provider_type(stored)?,
        provider_account_id(stored)?,
    ))
}

pub(crate) fn managed_account_binding_with_generation(
    stored: &StoredProvider,
) -> Option<(ProviderType, &str, u64)> {
    let (provider_type, account_id) = managed_account_binding(stored)?;
    Some((
        provider_type,
        account_id,
        provider_auth_identity_generation(stored)?,
    ))
}

pub(crate) fn authoritative_managed_account<'a>(
    stored: &StoredProvider,
    accounts: &'a AccountStore,
) -> Option<&'a Account> {
    let (provider_type, account_id, expected_generation) =
        managed_account_binding_with_generation(stored)?;
    accounts.accounts.iter().find(|account| {
        account.id == account_id
            && account.provider_type == provider_type
            && account.auth_identity_generation == expected_generation
    })
}

fn provider_auth_identity_generation(stored: &StoredProvider) -> Option<u64> {
    stored
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.auth_binding.as_ref())
        .and_then(|binding| binding.auth_identity_generation)
}

fn legacy_profile_id(app: AppKind) -> anyhow::Result<ProfileId> {
    ProfileId::parse(format!("{}.legacy_compat", app.as_str()))
}

fn legacy_driver_id(stored: &StoredProvider) -> anyhow::Result<DriverId> {
    if let Some(profile) =
        super::registry::profile_for_legacy_preset(stored.app, stored.provider.name.as_str())
    {
        if let DriverBinding::Fixed { driver_id } = &profile.driver_binding {
            return Ok(driver_id.clone());
        }
    }
    let api_format = stored
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.api_format.as_deref())
        .or_else(|| {
            stored
                .provider
                .settings_config
                .get("apiFormat")
                .and_then(Value::as_str)
        });
    if let Some(driver_id) = match api_format {
        Some("anthropic") => Some("http.anthropic_messages"),
        Some("openai_chat") => Some("http.openai_chat"),
        Some("openai_responses") => Some("http.openai_responses"),
        Some("gemini_native") => Some("http.gemini_native"),
        _ => None,
    } {
        return DriverId::parse(driver_id);
    }
    DriverId::parse(match stored.provider_type {
        ProviderType::Claude | ProviderType::ClaudeAuth => "http.anthropic_messages",
        ProviderType::ClaudeOAuth => "oauth.claude_messages",
        ProviderType::Codex => "http.openai_responses",
        ProviderType::CodexOAuth => "oauth.openai_codex",
        ProviderType::Gemini => "http.gemini_native",
        ProviderType::GeminiCli => "oauth.gemini_code_assist",
        ProviderType::OpenRouter => match stored.app {
            AppKind::Claude => "http.anthropic_messages",
            AppKind::Codex => "http.openai_responses",
            AppKind::Gemini => "http.openai_chat",
        },
        ProviderType::GitHubCopilot => "special.copilot",
        ProviderType::DeepSeekAccount => "special.deepseek_account",
        ProviderType::KiroOAuth => "special.kiro",
        ProviderType::KimiCode => "oauth.kimi_code",
        ProviderType::QoderCosy => "special.qoder_cosy",
        ProviderType::CursorOAuth | ProviderType::CursorApiKey => "special.cursor",
        ProviderType::AntigravityOAuth => "special.antigravity",
        ProviderType::AgyOAuth => "special.agy",
        ProviderType::OllamaCloud => "http.openai_chat",
        ProviderType::AwsBedrock => "aws.bedrock_sigv4",
        ProviderType::Nvidia | ProviderType::DeepSeekApi => match stored.app {
            AppKind::Claude if stored.provider_type == ProviderType::DeepSeekApi => {
                "http.anthropic_messages"
            }
            _ => "http.openai_chat",
        },
        ProviderType::GrokOAuth => "oauth.grok_responses",
    })
}

fn runtime_fingerprint(value: &Value) -> anyhow::Result<String> {
    let bytes =
        serde_json::to_vec(value).context("serialize Provider runtime fingerprint input")?;
    let digest = Sha256::digest(bytes);
    Ok(hex::encode(digest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::providers::model::{AuthBinding, Provider, ProviderMeta};
    use crate::domain::providers::registry::{CredentialPolicy, DriverBinding, ProfileSpec};
    use crate::domain::providers::store::ProviderResourceMetadata;

    #[test]
    fn provider_settings_defaults_use_seconds_and_luna() {
        let request = ProviderRequestDefaults::default();
        let health = ProviderHealthCheckConfig::default();
        let runtime = ProviderRuntimeDefaults::from_settings(&request, &health);

        assert_eq!(request.request_timeout_seconds, 300);
        assert_eq!(request.stream_first_byte_timeout_seconds, 120);
        assert_eq!(request.stream_idle_timeout_seconds, 300);
        assert_eq!(health.degraded_threshold_seconds, 6);
        assert_eq!(health.test_models.codex, "gpt-5.6-luna@low");
        assert_eq!(runtime.transport.timeout_ms, 300_000);
        assert_eq!(runtime.transport.stream_first_byte_timeout_ms, 120_000);
        assert_eq!(runtime.transport.stream_idle_timeout_ms, 300_000);
    }

    #[test]
    fn public_model_probes_match_each_app_wire_contract() {
        let claude = build_provider_model_probe(
            AppKind::Claude,
            ProviderType::Claude,
            "claude-sonnet-test",
            "ping",
            true,
            "claude-fingerprint",
        );
        assert_eq!(claude.api_type, "anthropic");
        assert_eq!(claude.path, "/v1/messages");
        assert_eq!(claude.body["model"], "claude-sonnet-test");
        assert_eq!(claude.body["stream"], true);
        assert_eq!(claude.response_mode, "anthropic_sse");

        let codex = build_provider_model_probe(
            AppKind::Codex,
            ProviderType::CodexOAuth,
            "gpt-5.6-luna@low",
            "ping",
            true,
            "codex-fingerprint",
        );
        assert_eq!(codex.api_type, "openai");
        assert_eq!(codex.path, "/v1/responses");
        assert_eq!(codex.requested_model, "gpt-5.6-luna@low");
        assert_eq!(codex.wire_model, "gpt-5.6-luna");
        assert_eq!(codex.body["model"], "gpt-5.6-luna");
        assert_eq!(codex.body["reasoning"]["effort"], "low");
        assert_eq!(codex.body["store"], false);
        assert_eq!(codex.response_mode, "responses_sse");

        let gemini = build_provider_model_probe(
            AppKind::Gemini,
            ProviderType::Gemini,
            "publishers/google/models/gemini-test",
            "ping",
            false,
            "gemini-fingerprint",
        );
        assert_eq!(gemini.api_type, "gemini");
        assert_eq!(
            gemini.path,
            "/v1beta/models/publishers%2Fgoogle%2Fmodels%2Fgemini-test:generateContent"
        );
        assert_eq!(gemini.body["contents"][0]["parts"][0]["text"], "ping");
        assert_eq!(gemini.response_mode, "json");
        assert_eq!(
            gemini.payload_revision,
            PROVIDER_MODEL_PROBE_PAYLOAD_REVISION
        );
    }

    #[test]
    fn non_codex_probe_model_modifiers_remain_literal_model_names() {
        let claude = build_provider_model_probe(
            AppKind::Claude,
            ProviderType::Claude,
            "claude-sonnet@2026-08-24",
            "ping",
            true,
            "claude-fingerprint",
        );
        assert_eq!(claude.wire_model, "claude-sonnet@2026-08-24");
        assert_eq!(claude.body["model"], "claude-sonnet@2026-08-24");
        assert!(claude.body.get("reasoning").is_none());

        let gemini = build_provider_model_probe(
            AppKind::Gemini,
            ProviderType::Gemini,
            "gemini-preview#2026-08-24",
            "ping",
            false,
            "gemini-fingerprint",
        );
        assert_eq!(gemini.wire_model, "gemini-preview#2026-08-24");
        assert_eq!(
            gemini.path,
            "/v1beta/models/gemini-preview%232026-08-24:generateContent"
        );
    }

    fn provider(profile_id: &str, provider_type: ProviderType) -> StoredProvider {
        let profile = profile_by_id(profile_id).unwrap();
        let model_mapping = match profile.model_policy {
            ModelPolicyKind::Passthrough => json!({"mode": "passthrough"}),
            ModelPolicyKind::Single => json!({
                "mode": "single",
                "upstreamModel": profile.default_upstream_model.as_deref().unwrap_or("gpt-test")
            }),
        };
        StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: "provider-1".to_string(),
                name: "Runtime fixture".to_string(),
                settings_config: json!({
                    "env": {"OPENAI_BASE_URL": "https://api.example.test/v1"},
                    "modelMapping": model_mapping
                }),
                category: None,
                meta: Some(ProviderMeta {
                    provider_type: Some(provider_type.as_str().to_string()),
                    ..Default::default()
                }),
                extra: Default::default(),
            },
            provider_type,
            provider_type_id: provider_type.as_str().to_string(),
            resource: ProviderResourceMetadata {
                profile_id: Some(ProfileId::parse(profile_id).unwrap()),
                profile_schema_revision: Some(1),
                revision: 7,
                ..Default::default()
            },
        }
    }

    #[test]
    fn static_runtime_fingerprint_ignores_display_fields() {
        let accounts = AccountStore::default();
        let mut stored = provider("codex.openrouter", ProviderType::OpenRouter);
        stored.resource.credential_generation = 3;
        stored.provider.settings_config["env"]["OPENAI_API_KEY"] = json!("secret");
        let first = compile_runtime_plan(&stored, &accounts).unwrap();
        stored.provider.name = "Renamed".to_string();
        let second = compile_runtime_plan(&stored, &accounts).unwrap();
        assert_eq!(first.runtime_fingerprint, second.runtime_fingerprint);
    }

    #[test]
    fn logical_static_secret_slot_accepts_a_canonical_provider_credential() {
        let mut stored = provider("codex.cursor_api_key", ProviderType::CursorApiKey);
        stored.provider.settings_config["apiKey"] = json!("cursor-secret");

        let plan = compile_runtime_plan(&stored, &AccountStore::default()).unwrap();

        assert_eq!(plan.configuration_state, RuntimeConfigurationState::Ready);
        assert!(matches!(
            plan.auth_ref,
            RuntimeAuthRef::StaticCredential { ref slots, .. }
                if slots.iter().any(|slot| slot == "/settingsConfig/apiKey")
        ));
    }

    #[test]
    fn coding_plan_quota_credentials_do_not_satisfy_the_inference_slot() {
        let mut stored = provider("codex.volcengine_coding_plan", ProviderType::Codex);
        stored.provider.settings_config["env"]["VOLC_ACCESS_KEY_ID"] = json!("access-key");
        stored.provider.settings_config["env"]["VOLC_SECRET_ACCESS_KEY"] = json!("secret-key");

        let plan = compile_runtime_plan(&stored, &AccountStore::default()).unwrap();

        assert_eq!(plan.auth_ref, RuntimeAuthRef::Missing);
        assert_eq!(
            plan.configuration_state,
            RuntimeConfigurationState::NeedsAttention
        );
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.contains("credential is not configured")));
    }

    #[test]
    fn managed_account_binding_ignores_stale_binding_on_static_profile() {
        let mut stored = provider("codex.openrouter", ProviderType::OpenRouter);
        stored.provider.meta.as_mut().unwrap().auth_binding = Some(AuthBinding {
            source: Some("account".to_string()),
            auth_provider: Some(ProviderType::OpenRouter.as_str().to_string()),
            account_id: Some("stale-account".to_string()),
            auth_identity_generation: Some(4),
        });

        assert_eq!(managed_account_provider_type(&stored), None);
        assert_eq!(managed_account_binding(&stored), None);
        assert!(authoritative_managed_account(&stored, &AccountStore::default()).is_none());
    }

    #[test]
    fn authoritative_managed_account_requires_matching_identity_generation() {
        let mut stored = provider("codex.openai_oauth", ProviderType::CodexOAuth);
        stored.provider.meta.as_mut().unwrap().auth_binding = Some(AuthBinding {
            source: Some("account".to_string()),
            auth_provider: Some(ProviderType::CodexOAuth.as_str().to_string()),
            account_id: Some("account-1".to_string()),
            auth_identity_generation: Some(4),
        });
        let mut accounts = AccountStore {
            accounts: vec![serde_json::from_value(json!({
                "id": "account-1",
                "providerType": "codex_oauth",
                "authIdentityGeneration": 4,
                "accessToken": "managed-access-token"
            }))
            .unwrap()],
            ..Default::default()
        };

        assert_eq!(
            authoritative_managed_account(&stored, &accounts).map(|account| account.id.as_str()),
            Some("account-1")
        );
        accounts.accounts.push(
            serde_json::from_value(json!({
                "id": "account-2",
                "providerType": "codex_oauth",
                "authIdentityGeneration": 4,
                "accessToken": "managed-access-token-2"
            }))
            .unwrap(),
        );
        assert_eq!(
            authoritative_managed_account(&stored, &accounts).map(|account| account.id.as_str()),
            Some("account-1")
        );
        accounts
            .select_active_codex_oauth_account("account-1")
            .unwrap();
        assert_eq!(
            authoritative_managed_account(&stored, &accounts).map(|account| account.id.as_str()),
            Some("account-1")
        );
        accounts
            .select_active_codex_oauth_account("account-2")
            .unwrap();
        assert_eq!(
            authoritative_managed_account(&stored, &accounts).map(|account| account.id.as_str()),
            Some("account-1")
        );
        accounts
            .select_active_codex_oauth_account("account-1")
            .unwrap();
        accounts.accounts[0].auth_identity_generation = 5;
        assert!(authoritative_managed_account(&stored, &accounts).is_none());
    }

    #[test]
    fn codex_managed_auth_ref_uses_the_explicit_bound_account() {
        let mut stored = provider("codex.openai_oauth", ProviderType::CodexOAuth);
        stored.provider.meta.as_mut().unwrap().auth_binding = Some(AuthBinding {
            source: Some("account".to_string()),
            auth_provider: Some(ProviderType::CodexOAuth.as_str().to_string()),
            account_id: Some("account-1".to_string()),
            auth_identity_generation: Some(1),
        });
        let mut accounts = AccountStore::default();
        for account_id in ["account-1", "account-2"] {
            accounts.upsert(
                serde_json::from_value(json!({
                    "id": account_id,
                    "providerType": "codex_oauth",
                    "accessToken": format!("managed-access-{account_id}")
                }))
                .unwrap(),
            );
        }

        let unselected = compile_runtime_plan(&stored, &accounts).unwrap();
        assert!(matches!(
            unselected.auth_ref,
            RuntimeAuthRef::ManagedAccount {
                ref account_id,
                expected_provider_type: ProviderType::CodexOAuth,
                auth_identity_generation: 1,
            } if account_id == "account-1"
        ));
        assert_eq!(
            unselected.configuration_state,
            RuntimeConfigurationState::Ready
        );

        accounts
            .select_active_codex_oauth_account("account-2")
            .unwrap();
        let non_active = compile_runtime_plan(&stored, &accounts).unwrap();
        assert!(matches!(
            non_active.auth_ref,
            RuntimeAuthRef::ManagedAccount {
                ref account_id,
                expected_provider_type: ProviderType::CodexOAuth,
                auth_identity_generation: 1,
            } if account_id == "account-1"
        ));

        accounts
            .select_active_codex_oauth_account("account-1")
            .unwrap();
        let active = compile_runtime_plan(&stored, &accounts).unwrap();
        assert!(matches!(
            active.auth_ref,
            RuntimeAuthRef::ManagedAccount {
                ref account_id,
                expected_provider_type: ProviderType::CodexOAuth,
                auth_identity_generation: 1,
            } if account_id == "account-1"
        ));
    }

    #[test]
    fn managed_account_binding_requires_explicit_legacy_binding() {
        let mut stored = provider("codex.openrouter", ProviderType::OpenRouter);
        stored.resource.profile_id = None;
        stored.provider_type = ProviderType::KiroOAuth;
        stored.provider_type_id = ProviderType::KiroOAuth.as_str().to_string();
        stored.provider.meta.as_mut().unwrap().auth_binding = None;

        assert_eq!(
            managed_account_provider_type(&stored),
            Some(ProviderType::KiroOAuth)
        );
        assert_eq!(managed_account_binding(&stored), None);
    }

    #[test]
    fn runtime_fingerprint_changes_for_model_and_credential_generation() {
        let accounts = AccountStore::default();
        let mut stored = provider("codex.openrouter", ProviderType::OpenRouter);
        stored.provider.settings_config["env"]["OPENAI_API_KEY"] = json!("secret");
        let first = compile_runtime_plan(&stored, &accounts).unwrap();
        stored.resource.credential_generation = 1;
        let credential = compile_runtime_plan(&stored, &accounts).unwrap();
        assert_ne!(first.runtime_fingerprint, credential.runtime_fingerprint);
        stored.provider.settings_config["modelMapping"]["upstreamModel"] = json!("gpt-next");
        let model = compile_runtime_plan(&stored, &accounts).unwrap();
        assert_ne!(credential.runtime_fingerprint, model.runtime_fingerprint);
    }

    #[test]
    fn profiled_runtime_uses_typed_transport_and_ignores_hidden_media_settings() {
        let accounts = AccountStore::default();
        let mut stored = provider("codex.openrouter", ProviderType::OpenRouter);
        stored.provider.settings_config["env"]["OPENAI_API_KEY"] = json!("secret");
        stored.provider.settings_config["env"]["UPSTREAM_TIMEOUT_MS"] = json!("999999");
        stored.provider.settings_config["transport"] = json!({
            "timeoutMs": 45_000,
            "streamFirstByteTimeoutMs": 15_000,
            "streamIdleTimeoutMs": 30_000,
        });
        stored.provider.settings_config["testModel"] = json!("health-model");
        stored.provider.settings_config["imageModel"] = json!("hidden-image-model");

        let first = compile_runtime_plan(&stored, &accounts).unwrap();

        assert_eq!(first.test_model.as_deref(), Some("health-model"));
        assert_eq!(first.transport_policy.timeout_ms, 45_000);
        assert_eq!(
            first.transport_policy.stream_first_byte_timeout_ms,
            Some(15_000)
        );
        assert_eq!(first.transport_policy.stream_idle_timeout_ms, Some(30_000));
        assert_eq!(first.media_policy, None);

        stored.provider.settings_config["testModel"] = json!("next-health-model");
        let changed = compile_runtime_plan(&stored, &accounts).unwrap();
        assert_eq!(first.runtime_fingerprint, changed.runtime_fingerprint);
        assert_ne!(first.health_fingerprint(), changed.health_fingerprint());
    }

    #[test]
    fn profiled_runtime_resolves_server_provider_and_surface_overrides() {
        let accounts = AccountStore::default();
        let mut stored = provider("codex.openrouter", ProviderType::OpenRouter);
        stored.provider.settings_config["env"]["OPENAI_API_KEY"] = json!("secret");
        stored.provider.settings_config["transport"] = json!({
            "timeoutMs": 75_000,
        });
        let defaults = ProviderRuntimeDefaults {
            transport: ProviderTransportDefaults {
                timeout_ms: 310_000,
                stream_first_byte_timeout_ms: 130_000,
                stream_idle_timeout_ms: 320_000,
            },
            test_models: ProviderTestModelDefaults {
                claude: "server-claude".to_string(),
                codex: "server-codex".to_string(),
                gemini: "server-gemini".to_string(),
            },
            probe_policy_fingerprint: "probe-policy".to_string(),
        };

        let inherited = compile_runtime_plan_with_defaults(&stored, &accounts, &defaults).unwrap();
        assert_eq!(inherited.test_model.as_deref(), Some("server-codex"));
        assert_eq!(inherited.transport_policy.timeout_ms, 75_000);
        assert_eq!(
            inherited.transport_policy.stream_first_byte_timeout_ms,
            Some(130_000)
        );
        assert_eq!(
            inherited.transport_policy.stream_idle_timeout_ms,
            Some(320_000)
        );

        stored
            .provider
            .extra
            .insert("testModel".to_string(), json!("provider-codex"));
        let provider_override =
            compile_runtime_plan_with_defaults(&stored, &accounts, &defaults).unwrap();
        assert_eq!(
            provider_override.test_model.as_deref(),
            Some("provider-codex")
        );
        assert_eq!(
            inherited.runtime_fingerprint,
            provider_override.runtime_fingerprint
        );
        assert_ne!(
            inherited.health_fingerprint(),
            provider_override.health_fingerprint()
        );

        stored.provider.settings_config["testModel"] = json!("surface-codex");
        let surface_override =
            compile_runtime_plan_with_defaults(&stored, &accounts, &defaults).unwrap();
        assert_eq!(
            surface_override.test_model.as_deref(),
            Some("surface-codex")
        );
    }

    #[test]
    fn legacy_runtime_keeps_frozen_transport_and_media_compatibility() {
        let accounts = AccountStore::default();
        let mut stored = provider("codex.openrouter", ProviderType::OpenRouter);
        stored.resource.profile_id = None;
        stored.resource.profile_schema_revision = None;
        stored.provider.settings_config["env"]["OPENAI_API_KEY"] = json!("secret");
        stored.provider.settings_config["env"]["UPSTREAM_TIMEOUT_MS"] = json!("42000");
        stored.provider.settings_config["imageModel"] = json!("legacy-image-model");

        let plan = compile_runtime_plan(&stored, &accounts).unwrap();

        assert_eq!(plan.transport_policy.timeout_ms, 42_000);
        assert_eq!(
            plan.media_policy,
            Some(json!({
                "imageModel": "legacy-image-model",
                "videoModel": null,
            }))
        );
    }

    #[test]
    fn every_registered_profile_compiles_into_the_runtime_index() {
        let mut accounts = AccountStore::default();
        let providers = provider_registry()
            .profiles
            .iter()
            .enumerate()
            .map(|(index, profile)| provider_for_profile(profile, index, &mut accounts))
            .collect::<Vec<_>>();
        let store = ProviderStore {
            providers,
            ..Default::default()
        };

        let index = ProviderRuntimeIndex::compile(&store, &accounts).unwrap();

        assert_eq!(index.len(), provider_registry().profiles.len());
        for profile in &provider_registry().profiles {
            let provider_id = format!("profile-{}", profile.profile_id.as_str().replace('.', "-"));
            let plan = index.get(profile.app, &provider_id).unwrap();
            assert_eq!(plan.profile_id, profile.profile_id);
            assert!(!plan.runtime_fingerprint.is_empty());
            assert!(plan.transport_policy.direct_connection);
        }
    }

    #[test]
    fn every_non_legacy_profile_that_allows_passthrough_compiles_it() {
        let mut accounts = AccountStore::default();
        for (index, profile) in provider_registry().profiles.iter().enumerate() {
            if profile.form_composition == super::super::registry::FormComposition::Legacy
                || !profile.allows_model_policy(ModelPolicyKind::Passthrough)
            {
                continue;
            }
            let mut stored = provider_for_profile(profile, index, &mut accounts);
            stored.provider.settings_config["modelMapping"] = json!({"mode": "passthrough"});

            let plan = compile_runtime_plan(&stored, &accounts).unwrap();

            assert_eq!(
                plan.model_policy,
                RuntimeModelPolicy::Passthrough,
                "{}",
                profile.profile_id
            );
        }
    }

    #[test]
    fn managed_runtime_fingerprint_ignores_token_refresh_but_tracks_identity() {
        let profile = profile_by_id("codex.openai_oauth").unwrap();
        let mut accounts = AccountStore::default();
        let stored = provider_for_profile(profile, 0, &mut accounts);
        let first = compile_runtime_plan(&stored, &accounts).unwrap();

        accounts.accounts[0].token_refresh_generation += 1;
        accounts.accounts[0].access_token = Some("refreshed-token".to_string());
        let refreshed = compile_runtime_plan(&stored, &accounts).unwrap();
        assert_eq!(first.runtime_fingerprint, refreshed.runtime_fingerprint);

        accounts.accounts[0].auth_identity_generation += 1;
        let changed_identity = compile_runtime_plan(&stored, &accounts).unwrap();
        assert_ne!(
            refreshed.runtime_fingerprint,
            changed_identity.runtime_fingerprint
        );
        assert_eq!(
            changed_identity.configuration_state,
            RuntimeConfigurationState::NeedsAttention
        );
        assert_eq!(changed_identity.auth_ref, RuntimeAuthRef::Missing);
    }

    #[test]
    fn bound_legacy_subscription_oauth_uses_managed_auth_even_with_a_static_secret() {
        let mut stored = provider("codex.openai_oauth", ProviderType::CodexOAuth);
        stored.resource.profile_id = None;
        stored.resource.profile_schema_revision = None;
        stored.provider.settings_config["env"]["OPENAI_API_KEY"] = json!("legacy-secret");
        stored.provider.meta.as_mut().unwrap().auth_binding = Some(AuthBinding {
            source: Some("account".to_string()),
            auth_provider: Some("codex_oauth".to_string()),
            account_id: Some("account-1".to_string()),
            auth_identity_generation: Some(3),
        });
        let account = serde_json::from_value(json!({
            "id": "account-1",
            "providerType": "codex_oauth",
            "authIdentityGeneration": 3,
            "accessToken": "managed-access-token"
        }))
        .unwrap();

        let plan = compile_runtime_plan(
            &stored,
            &AccountStore {
                accounts: vec![account],
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(
            plan.configuration_state,
            RuntimeConfigurationState::LegacyCompat
        );
        assert_eq!(
            plan.auth_ref,
            RuntimeAuthRef::ManagedAccount {
                account_id: "account-1".to_string(),
                expected_provider_type: ProviderType::CodexOAuth,
                auth_identity_generation: 3,
            }
        );
        assert!(has_authoritative_subscription_oauth_binding(&stored));
    }

    #[test]
    fn bound_legacy_grok_uses_managed_auth_even_with_a_static_secret() {
        let mut stored = provider("codex.grok_oauth", ProviderType::GrokOAuth);
        stored.resource.profile_id = None;
        stored.resource.profile_schema_revision = None;
        stored.provider.settings_config["env"]["OPENAI_API_KEY"] = json!("legacy-secret");
        stored.provider.settings_config["env"]["OPENAI_BASE_URL"] =
            json!("https://attacker.example/oauth");
        stored.provider.meta.as_mut().unwrap().auth_binding = Some(AuthBinding {
            source: Some("account".to_string()),
            auth_provider: Some("grok_oauth".to_string()),
            account_id: Some("grok-account".to_string()),
            auth_identity_generation: Some(3),
        });
        let account = serde_json::from_value(json!({
            "id": "grok-account",
            "providerType": "grok_oauth",
            "authIdentityGeneration": 3,
            "accessToken": "managed-grok-access-token"
        }))
        .unwrap();

        let plan = compile_runtime_plan(
            &stored,
            &AccountStore {
                accounts: vec![account],
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(plan.endpoint, "https://api.x.ai/v1");
        assert_eq!(
            plan.auth_ref,
            RuntimeAuthRef::ManagedAccount {
                account_id: "grok-account".to_string(),
                expected_provider_type: ProviderType::GrokOAuth,
                auth_identity_generation: 3,
            }
        );
        assert!(has_authoritative_subscription_oauth_binding(&stored));
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.contains("ignored a configured endpoint override")));
    }

    #[test]
    fn unbound_legacy_grok_rejects_a_static_secret() {
        let mut stored = provider("codex.grok_oauth", ProviderType::GrokOAuth);
        stored.resource.profile_id = None;
        stored.resource.profile_schema_revision = None;
        stored.provider.settings_config["env"]["OPENAI_API_KEY"] = json!("legacy-secret");

        let plan = compile_runtime_plan(&stored, &AccountStore::default()).unwrap();

        assert_eq!(
            plan.configuration_state,
            RuntimeConfigurationState::NeedsAttention
        );
        assert_eq!(plan.auth_ref, RuntimeAuthRef::Missing);
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.contains("must explicitly bind")));
    }

    #[test]
    fn custom_runtime_fingerprint_changes_for_endpoint_but_ignores_proxy_fields() {
        let accounts = AccountStore::default();
        let mut stored = provider("codex.custom_http", ProviderType::Codex);
        stored.resource.custom_binding = Some(super::super::registry::CustomBindingInput {
            upstream_protocol: UpstreamProtocol::OpenAiResponses,
            auth_scheme: AuthScheme::Bearer,
        });
        stored.provider.settings_config["env"]["OPENAI_API_KEY"] = json!("secret");
        let first = compile_runtime_plan(&stored, &accounts).unwrap();

        stored.provider.settings_config["env"]["OPENAI_BASE_URL"] =
            json!("https://next.example.test/v1");
        let endpoint = compile_runtime_plan(&stored, &accounts).unwrap();
        assert_ne!(first.runtime_fingerprint, endpoint.runtime_fingerprint);

        stored.provider.settings_config["proxy"] = json!("http://proxy.invalid:8080");
        stored.provider.settings_config["env"]["HTTPS_PROXY"] =
            json!("http://env-proxy.invalid:8080");
        let with_ignored_proxy = compile_runtime_plan(&stored, &accounts).unwrap();
        assert_eq!(
            endpoint.runtime_fingerprint,
            with_ignored_proxy.runtime_fingerprint
        );
        assert!(with_ignored_proxy.transport_policy.direct_connection);
        assert!(!serde_json::to_string(&with_ignored_proxy)
            .unwrap()
            .to_ascii_lowercase()
            .contains("proxy.invalid"));
    }

    #[test]
    fn anthropic_bearer_custom_profile_compiles_a_native_relay_plan() {
        let mut stored = provider("claude.custom_http", ProviderType::ClaudeAuth);
        stored.app = AppKind::Claude;
        stored.resource.custom_binding = Some(super::super::registry::CustomBindingInput {
            upstream_protocol: UpstreamProtocol::AnthropicMessages,
            auth_scheme: AuthScheme::Bearer,
        });
        stored.provider.settings_config = json!({
            "apiKey": "relay-secret",
            "env": {"ANTHROPIC_BASE_URL": "https://relay.example.test/v1"},
            "modelMapping": {"mode": "passthrough"}
        });

        let plan = compile_runtime_plan(&stored, &AccountStore::default()).unwrap();

        assert_eq!(plan.profile_id.as_str(), "claude.custom_http");
        assert_eq!(plan.driver_id.as_str(), "http.anthropic_messages");
        assert_eq!(plan.upstream_protocol, UpstreamProtocol::AnthropicMessages);
        assert_eq!(plan.endpoint, "https://relay.example.test/v1");
        assert_eq!(plan.model_policy, RuntimeModelPolicy::Passthrough);
        assert_eq!(plan.configuration_state, RuntimeConfigurationState::Ready);
        assert_eq!(
            plan.auth_ref,
            RuntimeAuthRef::CustomCredential {
                auth_scheme: AuthScheme::Bearer,
                slots: vec!["/settingsConfig/apiKey".to_string()],
                credential_generation: 0,
            }
        );
    }

    #[test]
    fn fixed_endpoint_policy_ignores_configured_override() {
        let accounts = AccountStore::default();
        let mut stored = provider("codex.openrouter", ProviderType::OpenRouter);
        stored.provider.settings_config["env"]["OPENAI_API_KEY"] = json!("secret");

        let first = compile_runtime_plan(&stored, &accounts).unwrap();
        assert_eq!(first.endpoint, "https://openrouter.ai/api");
        assert!(first
            .warnings
            .iter()
            .any(|warning| warning.contains("ignored a configured endpoint override")));

        stored.provider.settings_config["env"]["OPENAI_BASE_URL"] =
            json!("https://another.example.test/v1");
        let second = compile_runtime_plan(&stored, &accounts).unwrap();
        assert_eq!(second.endpoint, "https://openrouter.ai/api");
        assert_eq!(first.runtime_fingerprint, second.runtime_fingerprint);
    }

    #[test]
    fn configurable_profile_compiles_explicit_passthrough_policy() {
        let accounts = AccountStore::default();
        let mut stored = provider("codex.openrouter", ProviderType::OpenRouter);
        stored.provider.settings_config["env"]["OPENAI_API_KEY"] = json!("secret");
        stored.resource.credential_generation = 1;
        let single = compile_runtime_plan(&stored, &accounts).unwrap();
        stored.provider.settings_config["modelMapping"] = json!({"mode": "passthrough"});

        let plan = compile_runtime_plan(&stored, &accounts).unwrap();

        assert_eq!(plan.model_policy, RuntimeModelPolicy::Passthrough);
        assert_eq!(plan.configuration_state, RuntimeConfigurationState::Ready);
        assert_ne!(plan.runtime_fingerprint, single.runtime_fingerprint);
    }

    #[test]
    fn legacy_managed_oauth_providers_ignore_endpoint_overrides() {
        let accounts = AccountStore::default();
        for (app, provider_type, endpoint) in [
            (
                AppKind::Claude,
                ProviderType::ClaudeOAuth,
                "https://api.anthropic.com",
            ),
            (
                AppKind::Codex,
                ProviderType::CodexOAuth,
                "https://chatgpt.com/backend-api/codex",
            ),
            (
                AppKind::Codex,
                ProviderType::GrokOAuth,
                "https://api.x.ai/v1",
            ),
            (
                AppKind::Codex,
                ProviderType::CursorOAuth,
                "https://api2.cursor.sh",
            ),
            (
                AppKind::Codex,
                ProviderType::CursorApiKey,
                "https://api.cursor.com",
            ),
        ] {
            let mut stored = provider("codex.openai_oauth", provider_type);
            stored.app = app;
            stored.resource.profile_id = None;
            stored.resource.profile_schema_revision = None;
            stored.provider.settings_config = match app {
                AppKind::Claude => json!({
                    "env": {"ANTHROPIC_BASE_URL": "https://attacker.example/oauth"}
                }),
                AppKind::Codex => json!({
                    "env": {"OPENAI_BASE_URL": "https://attacker.example/oauth"}
                }),
                AppKind::Gemini => unreachable!(),
            };

            let plan = compile_runtime_plan(&stored, &accounts).unwrap();

            assert_eq!(plan.endpoint, endpoint);
            assert!(!plan.endpoint.contains("attacker.example"));
            assert!(plan
                .warnings
                .iter()
                .any(|warning| warning.contains("ignored a configured endpoint override")));
        }
    }

    #[test]
    fn failed_runtime_index_rebuild_keeps_the_committed_arc() {
        let accounts = AccountStore::default();
        let mut store = ProviderStore {
            providers: vec![provider("codex.openrouter", ProviderType::OpenRouter)],
            ..Default::default()
        };
        store.providers[0].provider.settings_config["env"]["OPENAI_API_KEY"] = json!("secret");
        store.rebuild_runtime_index(&accounts).unwrap();
        let committed = store.runtime_plan(AppKind::Codex, "provider-1").unwrap();

        store.providers[0].resource.profile_id = Some(ProfileId::parse("codex.unknown").unwrap());
        assert!(store.rebuild_runtime_index(&accounts).is_err());

        let retained = store.runtime_plan(AppKind::Codex, "provider-1").unwrap();
        assert!(Arc::ptr_eq(&committed, &retained));
    }

    #[test]
    fn custom_extra_headers_compile_as_secret_refs_without_values() {
        let accounts = AccountStore::default();
        let profile = profile_by_id("codex.custom_http").unwrap();
        let mut stored = provider_for_profile(profile, 7, &mut AccountStore::default());
        stored.resource.custom_binding = Some(super::super::registry::CustomBindingInput {
            upstream_protocol: UpstreamProtocol::OpenAiResponses,
            auth_scheme: AuthScheme::Bearer,
        });
        stored.provider.settings_config["env"]["OPENAI_API_KEY"] = json!("primary-secret");
        stored.provider.settings_config["extraHeaders"] = json!({
            "X-Tenant": "tenant-secret",
            "x-gateway-route": "route-secret"
        });

        let plan = compile_runtime_plan(&stored, &accounts).unwrap();

        assert_eq!(plan.configuration_state, RuntimeConfigurationState::Ready);
        assert_eq!(
            plan.extra_headers,
            vec![
                RuntimeExtraHeaderRef {
                    name: "x-gateway-route".to_string(),
                    credential_slot: "/settingsConfig/extraHeaders/x-gateway-route".to_string(),
                },
                RuntimeExtraHeaderRef {
                    name: "x-tenant".to_string(),
                    credential_slot: "/settingsConfig/extraHeaders/X-Tenant".to_string(),
                },
            ]
        );
        let serialized = serde_json::to_string(&plan).unwrap();
        assert!(!serialized.contains("tenant-secret"));
        assert!(!serialized.contains("route-secret"));
    }

    #[test]
    fn custom_extra_headers_cannot_replace_driver_controlled_headers() {
        let accounts = AccountStore::default();
        let profile = profile_by_id("codex.custom_http").unwrap();
        let mut stored = provider_for_profile(profile, 8, &mut AccountStore::default());
        stored.provider.settings_config["extraHeaders"] = json!({
            "Authorization": "override-secret"
        });

        let plan = compile_runtime_plan(&stored, &accounts).unwrap();

        assert_eq!(
            plan.configuration_state,
            RuntimeConfigurationState::NeedsAttention
        );
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.contains("controlled by the Provider driver")));
    }

    #[test]
    fn custom_auth_headers_allow_auth_fields_but_reject_transport_fields() {
        for name in ["Authorization", "x-api-key", "api-key", "x-goog-api-key"] {
            assert_eq!(
                validate_custom_auth_header_name(name).unwrap(),
                name.to_ascii_lowercase()
            );
            assert!(validate_custom_header_name(name).is_err());
        }
        for name in [
            "Host",
            "Content-Length",
            "Proxy-Authorization",
            "User-Agent",
        ] {
            assert!(validate_custom_auth_header_name(name).is_err());
        }
    }

    #[test]
    fn custom_extra_headers_cannot_bypass_the_user_agent_policy() {
        let accounts = AccountStore::default();
        let profile = profile_by_id("codex.custom_http").unwrap();
        let mut stored = provider_for_profile(profile, 10, &mut AccountStore::default());
        stored.provider.settings_config["extraHeaders"] = json!({
            "User-Agent": "shadow-agent/1"
        });

        let plan = compile_runtime_plan(&stored, &accounts).unwrap();

        assert_eq!(
            plan.configuration_state,
            RuntimeConfigurationState::NeedsAttention
        );
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.contains("user-agent")
                && warning.contains("controlled by the Provider driver")));
    }

    #[test]
    fn runtime_only_compiles_custom_user_agent_for_custom_profiles() {
        let accounts = AccountStore::default();
        let mut preset = provider("codex.openrouter", ProviderType::OpenRouter);
        preset.provider.settings_config["env"]["OPENAI_API_KEY"] = json!("secret");
        preset.provider.meta.as_mut().unwrap().custom_user_agent =
            Some("legacy-preset-override/1".to_string());
        let preset_plan = compile_runtime_plan(&preset, &accounts).unwrap();
        assert_eq!(
            preset_plan.outbound_identity_policy,
            OutboundIdentityPolicy::ServerIdentity
        );
        assert!(!preset_plan.driver_options.contains_key("customUserAgent"));

        let mut custom = provider("codex.custom_http", ProviderType::Codex);
        custom.resource.custom_binding = Some(super::super::registry::CustomBindingInput {
            upstream_protocol: UpstreamProtocol::OpenAiResponses,
            auth_scheme: AuthScheme::Bearer,
        });
        custom.provider.settings_config["env"]["OPENAI_API_KEY"] = json!("secret");
        custom.provider.meta.as_mut().unwrap().custom_user_agent =
            Some(" custom-agent/2 ".to_string());
        let custom_plan = compile_runtime_plan(&custom, &accounts).unwrap();
        assert_eq!(
            custom_plan.outbound_identity_policy,
            OutboundIdentityPolicy::CustomOverride
        );
        assert_eq!(
            custom_plan
                .driver_options
                .get("customUserAgent")
                .and_then(Value::as_str),
            Some("custom-agent/2")
        );
    }

    #[test]
    fn codex_runtime_compiles_keepalive_and_routing_options_from_provider_meta() {
        let accounts = AccountStore::default();
        let mut stored = provider("codex.openai_oauth", ProviderType::CodexOAuth);
        let meta = stored.provider.meta.as_mut().unwrap();
        meta.codex_responses_keepalive_interval_ms = Some(22_000);
        meta.codex_routing_hint_enabled = Some(false);

        let plan = compile_runtime_plan(&stored, &accounts).unwrap();

        assert_eq!(
            plan.driver_options
                .get("codexResponsesKeepaliveIntervalMs")
                .and_then(Value::as_u64),
            Some(22_000)
        );
        assert_eq!(
            plan.driver_options
                .get("codexRoutingHintEnabled")
                .and_then(Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn invalid_custom_user_agent_marks_the_runtime_plan_for_attention() {
        let accounts = AccountStore::default();
        let mut custom = provider("codex.custom_http", ProviderType::Codex);
        custom.resource.custom_binding = Some(super::super::registry::CustomBindingInput {
            upstream_protocol: UpstreamProtocol::OpenAiResponses,
            auth_scheme: AuthScheme::Bearer,
        });
        custom.provider.settings_config["env"]["OPENAI_API_KEY"] = json!("secret");
        custom.provider.meta.as_mut().unwrap().custom_user_agent =
            Some("agent/1\nforged: value".to_string());

        let plan = compile_runtime_plan(&custom, &accounts).unwrap();

        assert_eq!(
            plan.configuration_state,
            RuntimeConfigurationState::NeedsAttention
        );
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.contains("custom User-Agent")));
    }

    #[test]
    fn custom_extra_headers_do_not_satisfy_primary_auth_credential() {
        let accounts = AccountStore::default();
        let profile = profile_by_id("codex.custom_http").unwrap();
        let mut stored = provider_for_profile(profile, 9, &mut AccountStore::default());
        stored.provider.settings_config["env"] = json!({
            "OPENAI_BASE_URL": "https://api.example.test/v1"
        });
        stored.provider.settings_config["extraHeaders"] = json!({
            "X-Tenant": "tenant-secret"
        });

        let plan = compile_runtime_plan(&stored, &accounts).unwrap();

        assert_eq!(plan.auth_ref, RuntimeAuthRef::Missing);
        assert_eq!(
            plan.configuration_state,
            RuntimeConfigurationState::NeedsAttention
        );
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.contains("credential is not configured")));
    }

    fn provider_for_profile(
        profile: &ProfileSpec,
        index: usize,
        accounts: &mut AccountStore,
    ) -> StoredProvider {
        let provider_id = format!("profile-{}", profile.profile_id.as_str().replace('.', "-"));
        let provider_type = profile
            .compatibility_provider_type
            .unwrap_or(match profile.app {
                AppKind::Claude => ProviderType::Claude,
                AppKind::Codex => ProviderType::Codex,
                AppKind::Gemini => ProviderType::Gemini,
            });
        let base_url_key = match profile.app {
            AppKind::Claude => "ANTHROPIC_BASE_URL",
            AppKind::Codex => "OPENAI_BASE_URL",
            AppKind::Gemini => "GOOGLE_GEMINI_BASE_URL",
        };
        let mut meta = ProviderMeta {
            provider_type: Some(provider_type.as_str().to_string()),
            ..Default::default()
        };
        if let CredentialPolicy::ManagedAccount {
            account_provider_type,
        } = profile.credential_policy
        {
            let account_id = format!("account-{index}");
            let account = serde_json::from_value(json!({
                "id": account_id,
                "providerType": account_provider_type,
            }))
            .unwrap();
            accounts.accounts.push(account);
            meta.auth_binding = Some(AuthBinding {
                source: Some("account".to_string()),
                auth_provider: Some(account_provider_type.as_str().to_string()),
                account_id: Some(format!("account-{index}")),
                auth_identity_generation: Some(1),
            });
        }
        let custom_binding = match &profile.driver_binding {
            DriverBinding::Custom { .. } => Some(custom_binding_for_profile(profile)),
            DriverBinding::Fixed { .. } => None,
        };
        let model_mapping = match profile.model_policy {
            ModelPolicyKind::Passthrough => json!({"mode": "passthrough"}),
            ModelPolicyKind::Single => json!({
                "mode": "single",
                "upstreamModel": profile
                    .default_upstream_model
                    .as_deref()
                    .unwrap_or("fixture-model")
            }),
        };
        StoredProvider {
            app: profile.app,
            provider: Provider {
                id: provider_id,
                name: profile.label.clone(),
                settings_config: json!({
                    "env": {
                        base_url_key: format!("https://provider-{index}.example.test/v1"),
                        "TEST_API_KEY": "secret"
                    },
                    "modelMapping": model_mapping
                }),
                category: None,
                meta: Some(meta),
                extra: Default::default(),
            },
            provider_type,
            provider_type_id: provider_type.as_str().to_string(),
            resource: ProviderResourceMetadata {
                profile_id: Some(profile.profile_id.clone()),
                profile_schema_revision: Some(profile.profile_schema_revision),
                revision: 1,
                credential_generation: 1,
                custom_binding,
                create_request_id: None,
                cursor_verified_identity: None,
            },
        }
    }

    fn custom_binding_for_profile(
        profile: &ProfileSpec,
    ) -> super::super::registry::CustomBindingInput {
        let DriverBinding::Custom { custom_policy_id } = &profile.driver_binding else {
            unreachable!("custom binding helper requires a custom Profile");
        };
        let policy = provider_registry()
            .custom_policies
            .iter()
            .find(|policy| policy.custom_policy_id == *custom_policy_id)
            .unwrap();
        for upstream_protocol in &policy.protocols {
            for auth_scheme in &policy.auth_schemes {
                let input = super::super::registry::CustomBindingInput {
                    upstream_protocol: *upstream_protocol,
                    auth_scheme: *auth_scheme,
                };
                if resolve_custom_binding(profile, &input).is_ok() {
                    return input;
                }
            }
        }
        panic!(
            "custom Profile {} has no resolvable binding",
            profile.profile_id
        );
    }
}
