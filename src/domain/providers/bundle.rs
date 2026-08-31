use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::credentials::{CredentialPatch, ProviderView};
use super::model::{AppKind, Provider, ProviderMeta};
use super::model_routing::normalize_and_validate_provider_model_routing;
use super::registry::{
    family_by_id, family_for_profile, profile_by_id, provider_registry, resolve_custom_binding,
    AuthScheme, CredentialPolicy, CredentialSourceScope, CustomBindingInput, DriverBinding,
    EndpointPolicy, FormComposition, ModelPolicyKind, ProfileId, ProfileSpec, ProviderFamilySpec,
    UpstreamProtocol,
};
use super::store::StoredProvider;

const BUNDLE_ID_FIELD: &str = "bundleId";
const FAMILY_ID_FIELD: &str = "familyId";
const SURFACE_ENABLED_FIELD: &str = "surfaceEnabled";
const MODEL_POLICY_SCOPE_FIELD: &str = "modelPolicyScope";
const TEST_APP_FIELD: &str = "testApp";
const TEST_MODEL_FIELD: &str = "testModel";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPolicyScope {
    #[default]
    Global,
    PerApp,
}

impl ModelPolicyScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::PerApp => "per_app",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "global" => Some(Self::Global),
            "per_app" => Some(Self::PerApp),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPolicySource {
    BundleGlobal,
    AppIndependent,
    ProfileFixed,
}

impl ModelPolicySource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BundleGlobal => "bundle_global",
            Self::AppIndependent => "app_independent",
            Self::ProfileFixed => "profile_fixed",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderBundleView {
    pub id: String,
    pub family_id: String,
    pub revision: u64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub website_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_color: Option<String>,
    pub model_policy_scope: ModelPolicyScope,
    pub test_app: AppKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_model: Option<String>,
    pub surface_test_models: BTreeMap<AppKind, String>,
    pub transport: ProviderTransportWriteDraft,
    pub supported_apps: Vec<AppKind>,
    pub enabled_apps: Vec<AppKind>,
    pub credential_configured: bool,
    pub credential_slots: Vec<String>,
    pub surfaces: BTreeMap<AppKind, ProviderView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderBundleReferencePreview {
    pub bundle_id: String,
    pub revision: u64,
    pub share_ids: Vec<String>,
    pub blocked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderBundleWriteDraft {
    pub id: String,
    pub family_id: String,
    pub name: String,
    #[serde(default)]
    pub website_url: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub icon_color: Option<String>,
    #[serde(default)]
    pub model_policy_scope: ModelPolicyScope,
    #[serde(default)]
    pub model_policy: Option<ModelPolicyKind>,
    #[serde(default)]
    pub upstream_model: Option<String>,
    pub test_app: AppKind,
    #[serde(default)]
    pub test_model: Option<String>,
    #[serde(default)]
    pub surface_test_models: BTreeMap<AppKind, String>,
    #[serde(default)]
    pub transport: ProviderTransportWriteDraft,
    #[serde(default)]
    pub managed_account: Option<ProviderBundleManagedAccountWriteDraft>,
    #[serde(default)]
    pub aws_region: Option<String>,
    pub surfaces: Vec<ProviderBundleSurfaceWriteDraft>,
    #[serde(default)]
    pub credential_patches: BTreeMap<String, CredentialPatch>,
    #[serde(default)]
    pub expected_revision: Option<u64>,
    #[serde(default)]
    pub client_request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderBundleManagedAccountWriteDraft {
    pub account_id: String,
    pub auth_identity_generation: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderTransportWriteDraft {
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub stream_first_byte_timeout_ms: Option<u64>,
    #[serde(default)]
    pub stream_idle_timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderDriverOptionsWriteDraft {
    #[serde(default)]
    pub api_key_field: Option<String>,
    #[serde(default)]
    pub custom_user_agent: Option<String>,
    #[serde(default)]
    pub codex_fast_mode: Option<bool>,
    #[serde(default)]
    pub codex_image_generation_enabled: Option<bool>,
    #[serde(default)]
    pub grok_image_generation_enabled: Option<bool>,
    #[serde(default)]
    pub grok_image_edit_enabled: Option<bool>,
    #[serde(default)]
    pub grok_video_generation_enabled: Option<bool>,
    #[serde(default)]
    pub codex_websocket_enabled: Option<bool>,
    #[serde(default)]
    pub codex_responses_keepalive_interval_ms: Option<u64>,
    #[serde(default)]
    pub codex_routing_hint_enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderBundleSurfaceWriteDraft {
    pub app: AppKind,
    pub enabled: bool,
    pub profile_id: ProfileId,
    #[serde(default)]
    pub model_policy: Option<ModelPolicyKind>,
    #[serde(default)]
    pub upstream_model: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub driver_options: ProviderDriverOptionsWriteDraft,
    #[serde(default)]
    pub extra_headers: Vec<String>,
    #[serde(default)]
    pub custom_binding: Option<CustomBindingInput>,
    #[serde(default)]
    pub credential_patches: BTreeMap<String, CredentialPatch>,
}

impl ProviderBundleWriteDraft {
    pub fn validate(&self) -> anyhow::Result<&'static ProviderFamilySpec> {
        validate_bundle_id(&self.id)?;
        if self.name.trim().is_empty() || self.name != self.name.trim() {
            bail!("Provider Bundle name must be non-empty and trimmed");
        }
        let family = family_by_id(&self.family_id)
            .with_context(|| format!("unknown Provider family {}", self.family_id))?;
        if self.surfaces.len() != family.surfaces.len() {
            bail!(
                "Provider family {} requires exactly {} Surface drafts",
                family.family_id,
                family.surfaces.len()
            );
        }
        let mut apps = BTreeSet::new();
        let mut enabled = 0usize;
        self.validate_shared_configuration(family)?;
        for surface in &self.surfaces {
            if !apps.insert(surface.app) {
                bail!(
                    "Provider Bundle repeats the {} Surface",
                    surface.app.as_str()
                );
            }
            let Some(spec) = family
                .surfaces
                .iter()
                .find(|candidate| candidate.app == surface.app)
            else {
                bail!(
                    "Provider family {} does not support the {} Surface",
                    family.family_id,
                    surface.app.as_str()
                );
            };
            if spec.profile_id != surface.profile_id {
                bail!(
                    "Provider family {} requires profile {} for the {} Surface",
                    family.family_id,
                    spec.profile_id,
                    surface.app.as_str()
                );
            }
            let profile = profile_by_id(surface.profile_id.as_str())
                .with_context(|| format!("unknown Provider profile {}", surface.profile_id))?;
            self.validate_surface_configuration(surface, profile)?;
            let mut provider = self.provider_for_surface(surface)?;
            normalize_and_validate_provider_model_routing(
                surface.app,
                &mut provider,
                Some(profile),
            )?;
            enabled += usize::from(surface.enabled);
        }
        if enabled == 0 {
            bail!("Provider Bundle must enable at least one Surface");
        }
        Ok(family)
    }

    pub fn provider_for_surface(
        &self,
        surface: &ProviderBundleSurfaceWriteDraft,
    ) -> anyhow::Result<Provider> {
        let profile = profile_by_id(surface.profile_id.as_str())
            .with_context(|| format!("unknown Provider profile {}", surface.profile_id))?;
        let mut extra = BTreeMap::new();
        insert_optional_string(&mut extra, "websiteUrl", self.website_url.as_deref());
        insert_optional_string(&mut extra, "notes", self.notes.as_deref());
        insert_optional_string(&mut extra, "icon", self.icon.as_deref());
        insert_optional_string(&mut extra, "iconColor", self.icon_color.as_deref());
        extra.insert(BUNDLE_ID_FIELD.to_string(), Value::String(self.id.clone()));
        extra.insert(
            FAMILY_ID_FIELD.to_string(),
            Value::String(self.family_id.clone()),
        );
        extra.insert(
            SURFACE_ENABLED_FIELD.to_string(),
            Value::Bool(surface.enabled),
        );
        extra.insert(
            MODEL_POLICY_SCOPE_FIELD.to_string(),
            Value::String(self.model_policy_scope.as_str().to_string()),
        );
        extra.insert(
            TEST_APP_FIELD.to_string(),
            Value::String(self.test_app.as_str().to_string()),
        );
        insert_optional_string(&mut extra, TEST_MODEL_FIELD, self.test_model.as_deref());
        let mut settings = Map::new();
        settings.insert(
            "modelMapping".to_string(),
            self.model_mapping_value_for_surface(surface, profile)?,
        );

        let mut env = Map::new();
        if let Some(endpoint) = normalized_optional_string(surface.endpoint.as_deref()) {
            env.insert(
                endpoint_environment_key(surface.app).to_string(),
                Value::String(endpoint.trim_end_matches('/').to_string()),
            );
        }
        if let Some(region) = normalized_optional_string(self.aws_region.as_deref()) {
            env.insert("AWS_REGION".to_string(), Value::String(region));
        }
        let needs_env_credential_parent = matches!(
            &profile.credential_policy,
            CredentialPolicy::StaticSecret { slots, .. }
                if slots
                    .iter()
                    .any(|slot| slot.starts_with("/settingsConfig/env/"))
        );
        if !env.is_empty() || needs_env_credential_parent {
            settings.insert("env".to_string(), Value::Object(env));
        }
        if let Some(test_model) = self.surface_test_models.get(&surface.app) {
            settings.insert("testModel".to_string(), Value::String(test_model.clone()));
        }
        let transport = transport_value(&self.transport);
        if !transport.is_empty() {
            settings.insert("transport".to_string(), Value::Object(transport));
        }
        if !surface.extra_headers.is_empty() {
            settings.insert(
                "extraHeaders".to_string(),
                Value::Object(
                    surface
                        .extra_headers
                        .iter()
                        .map(|name| (name.trim().to_string(), Value::String(String::new())))
                        .collect(),
                ),
            );
        }

        let mut meta = ProviderMeta {
            provider_type: profile
                .compatibility_provider_type
                .map(|provider_type| provider_type.as_str().to_string()),
            api_format: resolved_protocol(profile, surface.custom_binding.as_ref())
                .and_then(api_format_for_protocol)
                .map(str::to_string),
            api_key_field: normalized_optional_string(
                surface.driver_options.api_key_field.as_deref(),
            ),
            custom_user_agent: normalized_optional_string(
                surface.driver_options.custom_user_agent.as_deref(),
            ),
            codex_fast_mode: surface.driver_options.codex_fast_mode,
            codex_image_generation_enabled: surface.driver_options.codex_image_generation_enabled,
            grok_image_generation_enabled: surface.driver_options.grok_image_generation_enabled,
            grok_image_edit_enabled: surface.driver_options.grok_image_edit_enabled,
            grok_video_generation_enabled: surface.driver_options.grok_video_generation_enabled,
            codex_websocket_enabled: surface.driver_options.codex_websocket_enabled,
            codex_responses_keepalive_interval_ms: surface
                .driver_options
                .codex_responses_keepalive_interval_ms,
            codex_routing_hint_enabled: surface.driver_options.codex_routing_hint_enabled,
            ..ProviderMeta::default()
        };
        if let Some(account) = self.managed_account.as_ref() {
            meta.auth_binding = Some(super::model::AuthBinding {
                source: Some(super::model::MANAGED_ACCOUNT_AUTH_BINDING_SOURCE.to_string()),
                auth_provider: profile
                    .compatibility_provider_type
                    .map(|provider_type| provider_type.as_str().to_string()),
                account_id: Some(account.account_id.clone()),
                auth_identity_generation: Some(account.auth_identity_generation),
            });
        }

        Ok(Provider {
            id: self.id.clone(),
            name: self.name.clone(),
            settings_config: Value::Object(settings),
            category: (profile.form_composition == FormComposition::Custom)
                .then(|| "custom".to_string()),
            meta: Some(meta),
            extra,
        })
    }

    fn validate_shared_configuration(&self, family: &ProviderFamilySpec) -> anyhow::Result<()> {
        self.validate_model_configuration(family)?;
        self.validate_test_configuration(family)?;
        validate_transport(&self.transport)?;

        let credential_profile = profile_by_id(family.credential_profile_id.as_str())
            .expect("Provider family credential profile is registry-validated");
        let managed = matches!(
            credential_profile.credential_policy,
            CredentialPolicy::ManagedAccount { .. }
        );
        match (managed, self.managed_account.as_ref()) {
            (true, Some(account)) => {
                if account.account_id.trim().is_empty()
                    || account.account_id != account.account_id.trim()
                {
                    bail!("managed Provider Bundle accountId must be non-empty and trimmed");
                }
            }
            (true, None) => bail!("managed Provider Bundle requires one account binding"),
            (false, Some(_)) => bail!("static Provider Bundle cannot bind a managed account"),
            (false, None) => {}
        }

        let aws = matches!(
            credential_profile.credential_policy,
            CredentialPolicy::Aws { .. }
        );
        match (aws, normalized_optional_string(self.aws_region.as_deref())) {
            (true, Some(region)) => validate_aws_region(&region)?,
            (true, None) => bail!("AWS Provider Bundle requires a region"),
            (false, Some(_)) => bail!("awsRegion is only valid for AWS Provider Bundles"),
            (false, None) => {}
        }
        Ok(())
    }

    fn validate_test_configuration(&self, family: &ProviderFamilySpec) -> anyhow::Result<()> {
        if !family
            .surfaces
            .iter()
            .any(|surface| surface.app == self.test_app)
        {
            bail!(
                "Provider family {} does not support the selected test App {}",
                family.family_id,
                self.test_app.as_str()
            );
        }
        if !self
            .surfaces
            .iter()
            .any(|surface| surface.app == self.test_app && surface.enabled)
        {
            bail!("Provider Bundle test App must be enabled");
        }
        if normalized_optional_string(self.test_model.as_deref())
            .is_some_and(|test_model| test_model.len() > 256)
        {
            bail!("Provider test model must be at most 256 characters");
        }
        for (app, model) in &self.surface_test_models {
            if !family.surfaces.iter().any(|surface| surface.app == *app) {
                bail!(
                    "Provider family {} does not support a test-model override for {}",
                    family.family_id,
                    app.as_str()
                );
            }
            if model.trim().is_empty() || model != model.trim() || model.len() > 256 {
                bail!(
                    "Provider Surface test model for {} must be non-empty, trimmed, and at most 256 characters",
                    app.as_str()
                );
            }
        }
        Ok(())
    }

    fn validate_model_configuration(&self, family: &ProviderFamilySpec) -> anyhow::Result<()> {
        match self.model_policy_scope {
            ModelPolicyScope::Global => {
                if family_requires_per_app_model_policy(family) {
                    bail!(
                        "Provider family {} requires per-app model policies",
                        family.family_id
                    );
                }
                let policy = self
                    .model_policy
                    .context("global Provider Bundle requires a model policy")?;
                validate_model_policy_fields(
                    policy,
                    self.upstream_model.as_deref(),
                    "Provider Bundle",
                )?;
                if self.surfaces.iter().any(|surface| {
                    surface.model_policy.is_some()
                        || normalized_optional_string(surface.upstream_model.as_deref()).is_some()
                }) {
                    bail!("global Provider Bundle cannot define Surface model policies");
                }
                let has_configurable_profile = family_has_configurable_model_profile(family);
                for surface in &family.surfaces {
                    let profile = profile_by_id(surface.profile_id.as_str())
                        .expect("Provider family profile is registry-validated");
                    if has_configurable_profile && !profile_has_configurable_model_policy(profile) {
                        continue;
                    }
                    if !profile.allows_model_policy(policy) {
                        bail!(
                            "Provider profile {} does not allow the selected model policy",
                            profile.profile_id
                        );
                    }
                }
            }
            ModelPolicyScope::PerApp => {
                if self.model_policy.is_some()
                    || normalized_optional_string(self.upstream_model.as_deref()).is_some()
                {
                    bail!("per-app Provider Bundle cannot define a global model policy");
                }
                if !family_supports_per_app_model_policy(family) {
                    bail!(
                        "Provider family {} does not support per-app model policies",
                        family.family_id
                    );
                }
                for surface in &self.surfaces {
                    let profile = profile_by_id(surface.profile_id.as_str())
                        .expect("Provider family profile is registry-validated");
                    if profile_has_configurable_model_policy(profile) {
                        let policy = surface.model_policy.with_context(|| {
                            format!(
                                "per-app Provider Bundle requires a model policy for {}",
                                surface.app.as_str()
                            )
                        })?;
                        if !profile.allows_model_policy(policy) {
                            bail!(
                                "Provider profile {} does not allow the selected model policy",
                                profile.profile_id
                            );
                        }
                        validate_model_policy_fields(
                            policy,
                            surface.upstream_model.as_deref(),
                            surface.app.as_str(),
                        )?;
                    } else if surface.model_policy.is_some()
                        || normalized_optional_string(surface.upstream_model.as_deref()).is_some()
                    {
                        bail!(
                            "Provider profile {} has a fixed model policy",
                            profile.profile_id
                        );
                    }
                }
            }
        }
        Ok(())
    }

    fn validate_surface_configuration(
        &self,
        surface: &ProviderBundleSurfaceWriteDraft,
        profile: &super::registry::ProfileSpec,
    ) -> anyhow::Result<()> {
        let endpoint = normalized_optional_string(surface.endpoint.as_deref());
        match profile.endpoint_policy {
            EndpointPolicy::Custom if surface.enabled && endpoint.is_none() => {
                bail!("custom Provider Surface requires an endpoint")
            }
            EndpointPolicy::Fixed | EndpointPolicy::Template if endpoint.is_some() => bail!(
                "Provider profile {} does not allow an endpoint override",
                profile.profile_id
            ),
            EndpointPolicy::FrozenLegacy => {
                bail!("legacy Provider profiles cannot be written through the Bundle API")
            }
            _ => {}
        }
        if let Some(endpoint) = endpoint.as_deref() {
            validate_endpoint(endpoint)?;
        }
        if surface.extra_headers.len() > 32 {
            bail!("custom Provider cannot define more than 32 extra headers");
        }
        let mut header_names = BTreeSet::new();
        for name in &surface.extra_headers {
            let canonical = super::runtime::validate_custom_header_name(name)?;
            if !header_names.insert(canonical) {
                bail!("custom Provider extra headers contain a duplicate name");
            }
        }
        if !surface.extra_headers.is_empty() && profile.form_composition != FormComposition::Custom
        {
            bail!("extra headers are only valid for Custom Providers");
        }

        let resolved_driver_id = match &profile.driver_binding {
            DriverBinding::Fixed { driver_id } => {
                if surface.custom_binding.is_some() {
                    bail!("fixed Provider profile cannot define a custom binding");
                }
                driver_id.clone()
            }
            DriverBinding::Custom { .. } => {
                let binding = surface
                    .custom_binding
                    .as_ref()
                    .context("custom Provider Surface requires a protocol/auth binding")?;
                resolve_custom_binding(profile, binding)?.driver_id
            }
        };
        let driver = provider_registry()
            .drivers
            .iter()
            .find(|driver| driver.driver_id == resolved_driver_id)
            .context("resolved Provider driver is not registered")?;
        super::registry::validate_driver_option_input(
            driver,
            &surface.driver_options.configured_fields(),
        )?;

        let api_key_field =
            normalized_optional_string(surface.driver_options.api_key_field.as_deref());
        if let Some(binding) = surface.custom_binding.as_ref() {
            if matches!(
                binding.auth_scheme,
                AuthScheme::CustomHeader | AuthScheme::Query
            ) && api_key_field.is_none()
            {
                bail!("custom_header and query authentication require apiKeyField");
            }
            if binding.auth_scheme == AuthScheme::CustomHeader {
                super::runtime::validate_custom_auth_header_name(
                    api_key_field.as_deref().expect("apiKeyField was checked"),
                )?;
            }
        } else if api_key_field.is_some() {
            bail!("apiKeyField is only valid for Custom Providers");
        }
        if surface.driver_options.custom_user_agent.is_some()
            && profile.form_composition != FormComposition::Custom
        {
            bail!("customUserAgent is only valid for Custom Providers");
        }
        Ok(())
    }

    fn model_mapping_value_for_surface(
        &self,
        surface: &ProviderBundleSurfaceWriteDraft,
        profile: &ProfileSpec,
    ) -> anyhow::Result<Value> {
        let (policy, upstream_model) = if !profile_has_configurable_model_policy(profile) {
            (
                profile.model_policy,
                profile.default_upstream_model.as_deref(),
            )
        } else {
            match self.model_policy_scope {
                ModelPolicyScope::Global => (
                    self.model_policy
                        .context("global Provider Bundle requires a model policy")?,
                    self.upstream_model.as_deref(),
                ),
                ModelPolicyScope::PerApp => (
                    surface.model_policy.with_context(|| {
                        format!(
                            "per-app Provider Bundle requires a model policy for {}",
                            surface.app.as_str()
                        )
                    })?,
                    surface.upstream_model.as_deref(),
                ),
            }
        };
        Ok(match policy {
            ModelPolicyKind::Passthrough => json!({"mode": "passthrough"}),
            ModelPolicyKind::Single => json!({
                "mode": "single",
                "upstreamModel": upstream_model.map(str::trim).unwrap_or_default(),
            }),
        })
    }
}

fn profile_has_configurable_model_policy(profile: &ProfileSpec) -> bool {
    profile.allowed_model_policies.len() > 1
}

fn family_has_configurable_model_profile(family: &ProviderFamilySpec) -> bool {
    family.surfaces.iter().any(|surface| {
        let profile = profile_by_id(surface.profile_id.as_str())
            .expect("Provider family profile is registry-validated");
        profile_has_configurable_model_policy(profile)
    })
}

fn family_configurable_model_profile_count(family: &ProviderFamilySpec) -> usize {
    family
        .surfaces
        .iter()
        .filter(|surface| {
            let profile = profile_by_id(surface.profile_id.as_str())
                .expect("Provider family profile is registry-validated");
            profile_has_configurable_model_policy(profile)
        })
        .count()
}

fn profile_model_policy_set(profile: &ProfileSpec) -> BTreeSet<ModelPolicyKind> {
    if profile.allowed_model_policies.is_empty() {
        BTreeSet::from([profile.model_policy])
    } else {
        profile.allowed_model_policies.iter().copied().collect()
    }
}

fn family_requires_per_app_model_policy(family: &ProviderFamilySpec) -> bool {
    let mut profiles = family.surfaces.iter().map(|surface| {
        profile_by_id(surface.profile_id.as_str())
            .expect("Provider family profile is registry-validated")
    });
    let Some(first) = profiles.next() else {
        return false;
    };
    let first_policies = profile_model_policy_set(first);
    profiles.any(|profile| profile_model_policy_set(profile) != first_policies)
}

fn family_supports_per_app_model_policy(family: &ProviderFamilySpec) -> bool {
    family_requires_per_app_model_policy(family)
        || family_configurable_model_profile_count(family) >= 2
}

fn validate_model_policy_fields(
    policy: ModelPolicyKind,
    upstream_model: Option<&str>,
    scope: &str,
) -> anyhow::Result<()> {
    match policy {
        ModelPolicyKind::Single => {
            if normalized_optional_string(upstream_model).is_none() {
                bail!("single-model {scope} requires an upstream model");
            }
        }
        ModelPolicyKind::Passthrough => {
            if normalized_optional_string(upstream_model).is_some() {
                bail!("passthrough {scope} cannot define an upstream model");
            }
        }
    }
    Ok(())
}

impl ProviderDriverOptionsWriteDraft {
    fn configured_fields(&self) -> BTreeSet<&'static str> {
        let mut fields = BTreeSet::new();
        if self.api_key_field.is_some() {
            fields.insert("apiKeyField");
        }
        if self.custom_user_agent.is_some() {
            fields.insert("customUserAgent");
        }
        if self.codex_fast_mode.is_some() {
            fields.insert("codexFastMode");
        }
        if self.codex_image_generation_enabled.is_some() {
            fields.insert("codexImageGenerationEnabled");
        }
        if self.grok_image_generation_enabled.is_some() {
            fields.insert("grokImageGenerationEnabled");
        }
        if self.grok_image_edit_enabled.is_some() {
            fields.insert("grokImageEditEnabled");
        }
        if self.grok_video_generation_enabled.is_some() {
            fields.insert("grokVideoGenerationEnabled");
        }
        if self.codex_websocket_enabled.is_some() {
            fields.insert("codexWebsocketEnabled");
        }
        if self.codex_responses_keepalive_interval_ms.is_some() {
            fields.insert("codexResponsesKeepaliveIntervalMs");
        }
        if self.codex_routing_hint_enabled.is_some() {
            fields.insert("codexRoutingHintEnabled");
        }
        fields
    }
}

fn normalized_optional_string(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn endpoint_environment_key(app: AppKind) -> &'static str {
    match app {
        AppKind::Claude => "ANTHROPIC_BASE_URL",
        AppKind::Codex => "OPENAI_BASE_URL",
        AppKind::Gemini => "GOOGLE_GEMINI_BASE_URL",
    }
}

fn validate_endpoint(endpoint: &str) -> anyhow::Result<()> {
    let parsed = url::Url::parse(endpoint).context("Provider endpoint is not a valid URL")?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.host_str().is_none()
    {
        bail!("Provider endpoint must be an HTTP(S) URL without userinfo");
    }
    Ok(())
}

fn validate_aws_region(region: &str) -> anyhow::Result<()> {
    if region.len() > 64
        || !region
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        bail!("AWS region is invalid");
    }
    Ok(())
}

fn validate_transport(transport: &ProviderTransportWriteDraft) -> anyhow::Result<()> {
    validate_duration("timeoutMs", transport.timeout_ms, 1_000, 3_600_000)?;
    validate_duration(
        "streamFirstByteTimeoutMs",
        transport.stream_first_byte_timeout_ms,
        1_000,
        600_000,
    )?;
    validate_duration(
        "streamIdleTimeoutMs",
        transport.stream_idle_timeout_ms,
        1_000,
        3_600_000,
    )?;
    Ok(())
}

fn validate_duration(name: &str, value: Option<u64>, min: u64, max: u64) -> anyhow::Result<()> {
    if value.is_some_and(|value| !(min..=max).contains(&value)) {
        bail!("{name} must be between {min} and {max}");
    }
    Ok(())
}

fn transport_value(transport: &ProviderTransportWriteDraft) -> Map<String, Value> {
    let mut value = Map::new();
    for (name, duration) in [
        ("timeoutMs", transport.timeout_ms),
        (
            "streamFirstByteTimeoutMs",
            transport.stream_first_byte_timeout_ms,
        ),
        ("streamIdleTimeoutMs", transport.stream_idle_timeout_ms),
    ] {
        if let Some(duration) = duration {
            value.insert(name.to_string(), Value::from(duration));
        }
    }
    value
}

fn resolved_protocol(
    profile: &super::registry::ProfileSpec,
    custom_binding: Option<&CustomBindingInput>,
) -> Option<UpstreamProtocol> {
    match &profile.driver_binding {
        DriverBinding::Fixed { driver_id } => provider_registry()
            .drivers
            .iter()
            .find(|driver| driver.driver_id == *driver_id)
            .map(|driver| driver.upstream_protocol),
        DriverBinding::Custom { .. } => custom_binding.map(|binding| binding.upstream_protocol),
    }
}

fn api_format_for_protocol(protocol: UpstreamProtocol) -> Option<&'static str> {
    match protocol {
        UpstreamProtocol::AnthropicMessages => Some("anthropic"),
        UpstreamProtocol::OpenAiChat => Some("openai_chat"),
        UpstreamProtocol::OpenAiResponses => Some("openai_responses"),
        UpstreamProtocol::GeminiNative => Some("gemini_native"),
        _ => None,
    }
}

impl ProviderBundleView {
    pub fn from_surface_views(surface_views: Vec<ProviderView>) -> anyhow::Result<Self> {
        let first = surface_views
            .first()
            .context("Provider Bundle has no Surfaces")?;
        let id = bundle_id(&first.provider)
            .context("Provider Bundle Surface has no explicit Bundle identity")?
            .to_string();
        let family_id = family_id_for_view(first)?;
        let name = first.provider.name.clone();
        let model_policy_scope = bundle_model_policy_scope(&first.provider)?
            .context("Provider Bundle Surface has no model policy scope")?;
        let test_app =
            bundle_test_app(&first.provider)?.context("Provider Bundle Surface has no test App")?;
        let test_model = bundle_test_model_override(&first.provider).map(str::to_string);
        let transport = provider_transport_override(&first.provider)?;
        let mut revision = 0u64;
        let mut surfaces = BTreeMap::new();
        let mut enabled_apps = Vec::new();
        let mut enabled_credential_states = Vec::new();
        let mut credential_slots = BTreeSet::new();
        for view in surface_views {
            if bundle_id(&view.provider) != Some(id.as_str())
                || family_id_for_view(&view)? != family_id
                || view.provider.name != name
                || bundle_model_policy_scope(&view.provider)? != Some(model_policy_scope)
                || bundle_test_app(&view.provider)? != Some(test_app)
                || bundle_test_model_override(&view.provider) != test_model.as_deref()
                || provider_transport_override(&view.provider)? != transport
            {
                bail!("Provider Bundle Surface metadata is inconsistent");
            }
            revision = revision.max(view.revision);
            if surface_enabled(&view.provider) {
                enabled_apps.push(view.app);
                enabled_credential_states.push(view.credential_configured);
            }
            credential_slots.extend(view.credential_slots.iter().cloned());
            if surfaces.insert(view.app, view).is_some() {
                bail!("Provider Bundle repeats a Surface");
            }
        }
        let family = family_by_id(&family_id)
            .with_context(|| format!("unknown Provider family {family_id}"))?;
        let actual_apps = surfaces.keys().copied().collect::<BTreeSet<_>>();
        let expected_apps = family
            .surfaces
            .iter()
            .map(|surface| surface.app)
            .collect::<BTreeSet<_>>();
        if actual_apps != expected_apps {
            bail!("Provider Bundle does not contain its complete Surface set");
        }
        if !enabled_apps.contains(&test_app) {
            bail!("Provider Bundle test App is not enabled");
        }
        let supported_apps = family.surfaces.iter().map(|surface| surface.app).collect();
        let credential_profile = profile_by_id(family.credential_profile_id.as_str())
            .with_context(|| {
                format!(
                    "unknown credential profile {}",
                    family.credential_profile_id
                )
            })?;
        let credential_configured = match &credential_profile.credential_policy {
            CredentialPolicy::ManagedAccount { .. } => {
                !enabled_apps.is_empty()
                    && enabled_apps.iter().all(|app| {
                        surfaces
                            .get(app)
                            .and_then(|view| view.provider.meta.as_ref())
                            .and_then(|meta| meta.auth_binding.as_ref())
                            .and_then(|binding| binding.account_id.as_deref())
                            .is_some_and(|account_id| !account_id.trim().is_empty())
                    })
            }
            _ => {
                !enabled_credential_states.is_empty()
                    && enabled_credential_states
                        .into_iter()
                        .all(|configured| configured)
            }
        };
        let provider = &surfaces
            .values()
            .next()
            .expect("surfaces is non-empty")
            .provider;
        let surface_test_models = surfaces
            .iter()
            .filter_map(|(app, view)| {
                non_empty_string(view.provider.settings_config.get("testModel"))
                    .map(|model| (*app, model))
            })
            .collect();
        Ok(Self {
            id,
            family_id,
            revision,
            name,
            website_url: extra_string(provider, "websiteUrl"),
            notes: extra_string(provider, "notes"),
            icon: extra_string(provider, "icon"),
            icon_color: extra_string(provider, "iconColor"),
            model_policy_scope,
            test_app,
            test_model,
            surface_test_models,
            transport,
            supported_apps,
            enabled_apps,
            credential_configured,
            credential_slots: credential_slots.into_iter().collect(),
            surfaces,
        })
    }
}

pub fn bundle_id(provider: &Provider) -> Option<&str> {
    extra_string_ref(provider, BUNDLE_ID_FIELD)
}

pub fn bundle_family_id(provider: &Provider) -> Option<&str> {
    extra_string_ref(provider, FAMILY_ID_FIELD)
}

pub fn bundle_supported_apps(provider: &Provider) -> Option<Vec<AppKind>> {
    let family_id = extra_string_ref(provider, FAMILY_ID_FIELD)?;
    let family = family_by_id(family_id)?;
    Some(family.surfaces.iter().map(|surface| surface.app).collect())
}

pub fn bundle_model_policy_scope(provider: &Provider) -> anyhow::Result<Option<ModelPolicyScope>> {
    if bundle_id(provider).is_none() {
        return Ok(None);
    }
    let value = extra_string_ref(provider, MODEL_POLICY_SCOPE_FIELD)
        .context("Provider Bundle Surface has no model policy scope")?;
    ModelPolicyScope::parse(value)
        .map(Some)
        .with_context(|| format!("invalid Provider Bundle model policy scope {value}"))
}

pub fn bundle_test_app(provider: &Provider) -> anyhow::Result<Option<AppKind>> {
    if bundle_id(provider).is_none() {
        return Ok(None);
    }
    let value = extra_string_ref(provider, TEST_APP_FIELD)
        .context("Provider Bundle Surface has no test App")?;
    match value {
        "claude" => Ok(Some(AppKind::Claude)),
        "codex" => Ok(Some(AppKind::Codex)),
        "gemini" => Ok(Some(AppKind::Gemini)),
        _ => bail!("invalid Provider Bundle test App {value}"),
    }
}

pub fn bundle_test_model_override(provider: &Provider) -> Option<&str> {
    extra_string_ref(provider, TEST_MODEL_FIELD)
}

fn provider_transport_override(provider: &Provider) -> anyhow::Result<ProviderTransportWriteDraft> {
    let Some(value) = provider.settings_config.get("transport") else {
        return Ok(ProviderTransportWriteDraft::default());
    };
    serde_json::from_value(value.clone()).context("Provider transport override is invalid")
}

fn non_empty_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn bundle_model_policy_source(
    provider: &Provider,
    profile: Option<&ProfileSpec>,
) -> anyhow::Result<ModelPolicySource> {
    if profile.is_some_and(|profile| !profile_has_configurable_model_policy(profile)) {
        return Ok(ModelPolicySource::ProfileFixed);
    }
    Ok(match bundle_model_policy_scope(provider)? {
        Some(ModelPolicyScope::Global) => ModelPolicySource::BundleGlobal,
        Some(ModelPolicyScope::PerApp) | None => ModelPolicySource::AppIndependent,
    })
}

pub fn has_bundle_managed_metadata(provider: &Provider) -> bool {
    [
        BUNDLE_ID_FIELD,
        FAMILY_ID_FIELD,
        SURFACE_ENABLED_FIELD,
        MODEL_POLICY_SCOPE_FIELD,
        TEST_APP_FIELD,
    ]
    .iter()
    .any(|field| provider.extra.contains_key(*field))
}

pub fn is_explicit_bundle_surface(provider: &Provider) -> bool {
    extra_string_ref(provider, BUNDLE_ID_FIELD).is_some()
        && extra_string_ref(provider, FAMILY_ID_FIELD).is_some()
        && provider.extra.contains_key(SURFACE_ENABLED_FIELD)
}

pub fn surface_enabled(provider: &Provider) -> bool {
    provider
        .extra
        .get(SURFACE_ENABLED_FIELD)
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

pub fn set_surface_enabled(provider: &mut Provider, enabled: bool) {
    provider
        .extra
        .insert(SURFACE_ENABLED_FIELD.to_string(), Value::Bool(enabled));
}

pub fn set_bundle_test_app(provider: &mut Provider, app: AppKind) {
    provider.extra.insert(
        TEST_APP_FIELD.to_string(),
        Value::String(app.as_str().to_string()),
    );
}

pub fn default_enabled_test_app(family_id: &str, enabled: &[AppKind]) -> Option<AppKind> {
    if family_id == "family.openai_oauth" && enabled.contains(&AppKind::Codex) {
        return Some(AppKind::Codex);
    }
    [AppKind::Codex, AppKind::Claude, AppKind::Gemini]
        .into_iter()
        .find(|app| enabled.contains(app))
}

pub fn credential_source_app(family: &ProviderFamilySpec) -> anyhow::Result<AppKind> {
    profile_by_id(family.credential_profile_id.as_str())
        .map(|profile| profile.app)
        .with_context(|| {
            format!(
                "Provider family {} has an unknown credential profile {}",
                family.family_id, family.credential_profile_id
            )
        })
}

pub fn shared_credential_source_key(
    stored: &StoredProvider,
) -> anyhow::Result<Option<super::registry::ProviderKey>> {
    if !is_explicit_bundle_surface(&stored.provider) {
        return Ok(None);
    }
    let Some(family_id) = extra_string_ref(&stored.provider, FAMILY_ID_FIELD) else {
        return Ok(None);
    };
    let family =
        family_by_id(family_id).with_context(|| format!("unknown Provider family {family_id}"))?;
    if family.credential_source_scope == CredentialSourceScope::Surface {
        return Ok(None);
    }
    let credential_profile =
        profile_by_id(family.credential_profile_id.as_str()).with_context(|| {
            format!(
                "Provider family {} has an unknown credential profile {}",
                family.family_id, family.credential_profile_id
            )
        })?;
    if !matches!(
        credential_profile.credential_policy,
        CredentialPolicy::StaticSecret { .. } | CredentialPolicy::Aws { .. }
    ) {
        return Ok(None);
    }
    Ok(Some(super::registry::ProviderKey::new(
        credential_source_app(family)?,
        bundle_id(&stored.provider).context("Provider Bundle Surface has no Bundle identity")?,
    )?))
}

pub fn family_id_for_view(view: &ProviderView) -> anyhow::Result<String> {
    if let Some(value) = extra_string_ref(&view.provider, FAMILY_ID_FIELD) {
        return Ok(value.to_string());
    }
    let profile_id = view
        .profile_id
        .as_ref()
        .context("Provider Surface has no profile identity")?;
    family_for_profile(profile_id.as_str())
        .map(|family| family.family_id.clone())
        .with_context(|| format!("profile {profile_id} has no Provider family"))
}

fn validate_bundle_id(value: &str) -> anyhow::Result<()> {
    if value.trim().is_empty() || value != value.trim() || value.len() > 128 {
        bail!("Provider Bundle id must be non-empty, trimmed, and at most 128 characters");
    }
    Ok(())
}

fn extra_string(provider: &Provider, key: &str) -> Option<String> {
    extra_string_ref(provider, key).map(str::to_string)
}

fn extra_string_ref<'a>(provider: &'a Provider, key: &str) -> Option<&'a str> {
    provider
        .extra
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn insert_optional_string(target: &mut BTreeMap<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        target.insert(key.to_string(), Value::String(value.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn grok_surface(app: AppKind, profile_id: &str) -> ProviderBundleSurfaceWriteDraft {
        ProviderBundleSurfaceWriteDraft {
            app,
            enabled: true,
            profile_id: ProfileId::parse(profile_id).unwrap(),
            model_policy: None,
            upstream_model: None,
            endpoint: None,
            driver_options: ProviderDriverOptionsWriteDraft::default(),
            extra_headers: Vec::new(),
            custom_binding: None,
            credential_patches: BTreeMap::new(),
        }
    }

    fn grok_bundle() -> ProviderBundleWriteDraft {
        ProviderBundleWriteDraft {
            id: "grok-bundle".to_string(),
            family_id: "family.grok_oauth".to_string(),
            name: "Grok Bundle".to_string(),
            website_url: None,
            notes: None,
            icon: None,
            icon_color: None,
            model_policy_scope: ModelPolicyScope::Global,
            model_policy: Some(ModelPolicyKind::Single),
            upstream_model: Some("grok-4.6".to_string()),
            test_app: AppKind::Claude,
            test_model: None,
            surface_test_models: BTreeMap::new(),
            transport: ProviderTransportWriteDraft::default(),
            managed_account: Some(ProviderBundleManagedAccountWriteDraft {
                account_id: "grok-account".to_string(),
                auth_identity_generation: 1,
            }),
            aws_region: None,
            surfaces: vec![
                grok_surface(AppKind::Claude, "claude.grok_oauth"),
                grok_surface(AppKind::Codex, "codex.grok_oauth"),
                grok_surface(AppKind::Gemini, "gemini.grok_oauth"),
            ],
            credential_patches: BTreeMap::new(),
            expected_revision: None,
            client_request_id: None,
        }
    }

    fn openai_oauth_bundle() -> ProviderBundleWriteDraft {
        let mut claude = grok_surface(AppKind::Claude, "claude.openai_oauth");
        claude.model_policy = Some(ModelPolicyKind::Single);
        claude.upstream_model = Some("gpt-5.6-sol".to_string());
        let mut codex = grok_surface(AppKind::Codex, "codex.openai_oauth");
        codex.model_policy = Some(ModelPolicyKind::Passthrough);
        ProviderBundleWriteDraft {
            id: "openai-oauth-bundle".to_string(),
            family_id: "family.openai_oauth".to_string(),
            name: "OpenAI OAuth".to_string(),
            website_url: None,
            notes: None,
            icon: None,
            icon_color: None,
            model_policy_scope: ModelPolicyScope::PerApp,
            model_policy: None,
            upstream_model: None,
            test_app: AppKind::Codex,
            test_model: None,
            surface_test_models: BTreeMap::new(),
            transport: ProviderTransportWriteDraft::default(),
            managed_account: Some(ProviderBundleManagedAccountWriteDraft {
                account_id: "openai-account".to_string(),
                auth_identity_generation: 1,
            }),
            aws_region: None,
            surfaces: vec![claude, codex],
            credential_patches: BTreeMap::new(),
            expected_revision: None,
            client_request_id: None,
        }
    }

    #[test]
    fn bundle_generates_one_canonical_model_mapping_for_every_surface() {
        let draft = grok_bundle();
        assert!(draft.validate().is_ok());
        for surface in &draft.surfaces {
            assert_eq!(
                draft.provider_for_surface(surface).unwrap().settings_config["modelMapping"],
                json!({"mode": "single", "upstreamModel": "grok-4.6"})
            );
        }
    }

    #[test]
    fn bundle_materializes_provider_and_surface_test_model_overrides() {
        let mut draft = grok_bundle();
        draft.test_app = AppKind::Codex;
        draft.test_model = Some("grok-health".to_string());
        draft
            .surface_test_models
            .insert(AppKind::Claude, "claude-health".to_string());
        draft.transport.timeout_ms = Some(75_000);
        draft.transport.stream_idle_timeout_ms = Some(90_000);

        assert!(draft.validate().is_ok());
        for surface in &draft.surfaces {
            let provider = draft.provider_for_surface(surface).unwrap();
            if surface.app == AppKind::Claude {
                assert_eq!(provider.settings_config["testModel"], "claude-health");
            } else {
                assert!(provider.settings_config.get("testModel").is_none());
            }
            assert_eq!(provider.extra[TEST_APP_FIELD], "codex");
            assert_eq!(provider.extra[TEST_MODEL_FIELD], "grok-health");
            assert_eq!(
                provider.settings_config["transport"],
                json!({"timeoutMs": 75_000, "streamIdleTimeoutMs": 90_000})
            );
        }
    }

    #[test]
    fn bundle_rejects_a_disabled_test_app() {
        let mut draft = grok_bundle();
        draft.surfaces[0].enabled = false;

        assert!(draft
            .validate()
            .unwrap_err()
            .to_string()
            .contains("test App must be enabled"));
    }

    #[test]
    fn passthrough_bundle_rejects_an_upstream_model() {
        let mut draft = grok_bundle();
        draft.model_policy = Some(ModelPolicyKind::Passthrough);
        let error = draft.validate().unwrap_err();
        assert!(error
            .to_string()
            .contains("passthrough Provider Bundle cannot define an upstream model"));
    }

    #[test]
    fn openai_oauth_keeps_codex_passthrough_while_claude_is_configurable() {
        let mut draft = openai_oauth_bundle();
        assert!(draft.validate().is_ok());
        assert_eq!(
            draft
                .provider_for_surface(&draft.surfaces[0])
                .unwrap()
                .settings_config["modelMapping"],
            json!({"mode": "single", "upstreamModel": "gpt-5.6-sol"})
        );
        assert_eq!(
            draft
                .provider_for_surface(&draft.surfaces[1])
                .unwrap()
                .settings_config["modelMapping"],
            json!({"mode": "passthrough"})
        );

        draft.surfaces[0].model_policy = Some(ModelPolicyKind::Passthrough);
        draft.surfaces[0].upstream_model = None;
        assert!(draft.validate().is_ok());
        for surface in &draft.surfaces {
            assert_eq!(
                draft.provider_for_surface(surface).unwrap().settings_config["modelMapping"],
                json!({"mode": "passthrough"})
            );
        }

        draft.model_policy_scope = ModelPolicyScope::Global;
        draft.model_policy = Some(ModelPolicyKind::Passthrough);
        draft.upstream_model = None;
        for surface in &mut draft.surfaces {
            surface.model_policy = None;
            surface.upstream_model = None;
        }
        assert!(draft.validate().is_ok());
        for surface in &draft.surfaces {
            assert_eq!(
                draft.provider_for_surface(surface).unwrap().settings_config["modelMapping"],
                json!({"mode": "passthrough"})
            );
        }
    }

    #[test]
    fn openai_oauth_bundle_materializes_keepalive_and_routing_driver_options() {
        let mut draft = openai_oauth_bundle();
        for surface in &mut draft.surfaces {
            surface.driver_options.codex_responses_keepalive_interval_ms = Some(22_000);
            surface.driver_options.codex_routing_hint_enabled = Some(false);
        }

        assert!(draft.validate().is_ok());
        for surface in &draft.surfaces {
            let provider = draft.provider_for_surface(surface).unwrap();
            let meta = provider.meta.unwrap();
            assert_eq!(meta.codex_responses_keepalive_interval_ms, Some(22_000));
            assert_eq!(meta.codex_routing_hint_enabled, Some(false));
        }
    }

    #[test]
    fn per_app_bundle_materializes_each_surface_model_policy() {
        let mut draft = grok_bundle();
        draft.model_policy_scope = ModelPolicyScope::PerApp;
        draft.model_policy = None;
        draft.upstream_model = None;
        draft.surfaces[0].model_policy = Some(ModelPolicyKind::Single);
        draft.surfaces[0].upstream_model = Some("claude-model".to_string());
        draft.surfaces[1].model_policy = Some(ModelPolicyKind::Passthrough);
        draft.surfaces[2].model_policy = Some(ModelPolicyKind::Single);
        draft.surfaces[2].upstream_model = Some("gemini-model".to_string());

        assert!(draft.validate().is_ok());
        assert_eq!(
            draft
                .provider_for_surface(&draft.surfaces[0])
                .unwrap()
                .settings_config["modelMapping"],
            json!({"mode": "single", "upstreamModel": "claude-model"})
        );
        assert_eq!(
            draft
                .provider_for_surface(&draft.surfaces[1])
                .unwrap()
                .settings_config["modelMapping"],
            json!({"mode": "passthrough"})
        );
        assert_eq!(
            draft
                .provider_for_surface(&draft.surfaces[2])
                .unwrap()
                .settings_config["modelMapping"],
            json!({"mode": "single", "upstreamModel": "gemini-model"})
        );
    }

    #[test]
    fn per_app_bundle_rejects_global_model_fields() {
        let mut draft = grok_bundle();
        draft.model_policy_scope = ModelPolicyScope::PerApp;
        let error = draft.validate().unwrap_err();
        assert!(error
            .to_string()
            .contains("per-app Provider Bundle cannot define a global model policy"));
    }
}
