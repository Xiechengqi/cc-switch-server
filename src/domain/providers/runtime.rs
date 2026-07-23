use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{bail, Context};
use http::HeaderName;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use url::Url;

use crate::domain::accounts::store::AccountStore;

use super::credentials::redact_provider;
use super::model::{AppKind, Provider, ProviderType};
use super::model_routing::{policy_from_settings, ModelRoutingMode};
use super::registry::{
    profile_by_id, provider_registry, resolve_custom_binding, AuthScheme, CredentialPolicy,
    DriverBinding, DriverId, EndpointPolicy, ModelPolicyKind, OutboundIdentityPolicy, ProfileId,
    UpstreamProtocol,
};
use super::store::{ProviderStore, StoredProvider};

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
        Self {
            timeout_ms: 300_000,
            stream_first_byte_timeout_ms: Some(120_000),
            stream_idle_timeout_ms: Some(300_000),
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

#[derive(Debug, Clone, Default)]
pub struct ProviderRuntimeIndex {
    plans: BTreeMap<super::registry::ProviderKey, Arc<ProviderRuntimePlan>>,
}

impl ProviderRuntimeIndex {
    pub fn compile(store: &ProviderStore, accounts: &AccountStore) -> anyhow::Result<Self> {
        let mut plans = BTreeMap::new();
        for stored in &store.providers {
            let plan = Arc::new(compile_runtime_plan(stored, accounts)?);
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
    let endpoint = match endpoint_policy {
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

    let model_policy =
        runtime_model_policy(stored, profile_policy.map(|profile| profile.model_policy));
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
    let media_policy = runtime_media_policy(&stored.provider);
    let transport_policy = runtime_transport_policy(&stored.provider);
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
        "modelPolicy": &model_policy,
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
    let name = name.trim();
    if name.is_empty() {
        bail!("custom header name cannot be empty");
    }
    let parsed = HeaderName::from_bytes(name.as_bytes())
        .with_context(|| format!("custom header name is invalid: {name}"))?;
    let canonical = parsed.as_str().to_string();
    if matches!(
        canonical.as_str(),
        "authorization"
            | "proxy-authorization"
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
            | "x-api-key"
            | "api-key"
            | "x-goog-api-key"
            | "user-agent"
    ) {
        bail!("custom header {canonical} is controlled by the Provider driver");
    }
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
        return RuntimeAuthRef::Legacy {
            account_id: provider_account_id(stored).map(str::to_string),
            credential_generation: stored.resource.credential_generation,
        };
    };
    match &profile.credential_policy {
        CredentialPolicy::ManagedAccount {
            account_provider_type,
        } => {
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
            if account.provider_type != *account_provider_type {
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
                expected_provider_type: *account_provider_type,
                auth_identity_generation: expected_generation,
            }
        }
        CredentialPolicy::StaticSecret { slots, auth_scheme } => {
            let summary = redact_provider(&stored.provider).1;
            if !summary.configured {
                warnings.push("Provider credential is not configured".to_string());
                return RuntimeAuthRef::Missing;
            }
            RuntimeAuthRef::StaticCredential {
                auth_scheme: *auth_scheme,
                slots: normalized_slots(slots, &summary.slots),
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
    declared: Option<ModelPolicyKind>,
) -> anyhow::Result<RuntimeModelPolicy> {
    let configured = policy_from_settings(&stored.provider.settings_config);
    let kind = declared.unwrap_or_else(|| match configured.as_ref().map(|policy| policy.mode) {
        Some(ModelRoutingMode::Single) => ModelPolicyKind::Single,
        _ => ModelPolicyKind::Passthrough,
    });
    match kind {
        ModelPolicyKind::Passthrough => Ok(RuntimeModelPolicy::Passthrough),
        ModelPolicyKind::Single => runtime_single_model(&stored.provider.settings_config)
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
    ] {
        if let Some(value) = value {
            options.insert(name.to_string(), value);
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

fn runtime_media_policy(provider: &Provider) -> Option<Value> {
    let image_model = non_empty_value(provider.settings_config.get("imageModel"));
    let video_model = non_empty_value(provider.settings_config.get("videoModel"));
    (image_model.is_some() || video_model.is_some()).then(|| {
        json!({
            "imageModel": image_model,
            "videoModel": video_model,
        })
    })
}

fn runtime_transport_policy(provider: &Provider) -> RuntimeTransportPolicy {
    RuntimeTransportPolicy {
        timeout_ms: configured_timeout_ms(
            provider,
            &[
                "UPSTREAM_TIMEOUT_MS",
                "PROXY_TIMEOUT_MS",
                "REQUEST_TIMEOUT_MS",
            ],
            300_000,
        )
        .unwrap_or(300_000),
        stream_first_byte_timeout_ms: configured_timeout_ms(
            provider,
            &[
                "STREAM_FIRST_BYTE_TIMEOUT_MS",
                "UPSTREAM_STREAM_FIRST_BYTE_TIMEOUT_MS",
                "FIRST_BYTE_TIMEOUT_MS",
            ],
            120_000,
        ),
        stream_idle_timeout_ms: configured_timeout_ms(
            provider,
            &[
                "STREAM_IDLE_TIMEOUT_MS",
                "UPSTREAM_STREAM_IDLE_TIMEOUT_MS",
                "IDLE_TIMEOUT_MS",
            ],
            300_000,
        ),
        ..RuntimeTransportPolicy::default()
    }
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
        ProviderType::Claude | ProviderType::ClaudeOAuth => Some("https://api.anthropic.com"),
        ProviderType::Codex => Some("https://api.openai.com"),
        ProviderType::CodexOAuth => Some("https://chatgpt.com/backend-api/codex"),
        ProviderType::Gemini | ProviderType::GeminiCli => {
            Some("https://generativelanguage.googleapis.com")
        }
        ProviderType::AntigravityOAuth | ProviderType::AgyOAuth => {
            Some("https://daily-cloudcode-pa.googleapis.com")
        }
        ProviderType::OpenRouter => Some("https://openrouter.ai/api"),
        ProviderType::GitHubCopilot => Some("https://api.githubcopilot.com"),
        ProviderType::DeepSeekAccount => Some("https://chat.deepseek.com"),
        ProviderType::KiroOAuth => Some("https://q.us-east-1.amazonaws.com"),
        ProviderType::CursorOAuth => Some("https://api2.cursor.sh"),
        ProviderType::CursorApiKey => Some("https://api.cursor.com"),
        ProviderType::OllamaCloud => Some("https://ollama.com"),
        ProviderType::AwsBedrock => Some("https://bedrock-runtime.${AWS_REGION}.amazonaws.com"),
        ProviderType::Nvidia => Some("https://integrate.api.nvidia.com"),
        ProviderType::DeepSeekApi => Some("https://api.deepseek.com"),
        ProviderType::GrokOAuth => Some("https://api.x.ai/v1"),
        ProviderType::ClaudeAuth => None,
    }
}

fn managed_oauth_endpoint_is_fixed(provider_type: ProviderType) -> bool {
    matches!(
        provider_type,
        ProviderType::ClaudeOAuth | ProviderType::CodexOAuth
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
        ProviderType::GeminiCli => "oauth.gemini_native",
        ProviderType::OpenRouter => match stored.app {
            AppKind::Claude => "http.anthropic_messages",
            AppKind::Codex => "http.openai_responses",
            AppKind::Gemini => "http.openai_chat",
        },
        ProviderType::GitHubCopilot => "special.copilot",
        ProviderType::DeepSeekAccount => "special.deepseek_account",
        ProviderType::KiroOAuth => "special.kiro",
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

    fn provider(profile_id: &str, provider_type: ProviderType) -> StoredProvider {
        StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: "provider-1".to_string(),
                name: "Runtime fixture".to_string(),
                settings_config: json!({
                    "env": {"OPENAI_BASE_URL": "https://api.example.test/v1"},
                    "modelMapping": {"mode": "single", "upstreamModel": "gpt-test"}
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
                    "modelMapping": {
                        "mode": "single",
                        "upstreamModel": "fixture-model"
                    }
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
