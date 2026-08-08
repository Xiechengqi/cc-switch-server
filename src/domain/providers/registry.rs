use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};

use super::model::{AppKind, ProviderType};

pub const PROVIDER_REGISTRY_SCHEMA_VERSION: u32 = 5;
pub const PROVIDER_REGISTRY_FORMAT: &str = "cc-switch-provider-registry";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProfileId(String);

impl ProfileId {
    pub fn parse(value: impl Into<String>) -> anyhow::Result<Self> {
        let value = value.into();
        validate_registry_id(&value, "profile")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProfileId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DriverId(String);

impl DriverId {
    pub fn parse(value: impl Into<String>) -> anyhow::Result<Self> {
        let value = value.into();
        validate_registry_id(&value, "driver")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DriverId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderKey {
    pub app: AppKind,
    pub provider_id: String,
}

impl ProviderKey {
    pub fn new(app: AppKind, provider_id: impl Into<String>) -> anyhow::Result<Self> {
        let provider_id = provider_id.into();
        if provider_id.trim().is_empty() || provider_id != provider_id.trim() {
            bail!("provider id must be non-empty and trimmed");
        }
        Ok(Self { app, provider_id })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderRegistry {
    pub format: String,
    pub schema_version: u32,
    pub families: Vec<ProviderFamilySpec>,
    pub profiles: Vec<ProfileSpec>,
    pub drivers: Vec<DriverSpec>,
    pub option_schemas: Vec<DriverOptionSchemaSpec>,
    #[serde(default)]
    pub custom_policies: Vec<CustomPolicySpec>,
    #[serde(default)]
    pub legacy_preset_mappings: Vec<LegacyPresetMapping>,
    #[serde(default)]
    pub published_id_tombstones: Vec<PublishedIdTombstone>,
    #[serde(default)]
    pub conformance: Vec<DriverConformance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DriverOptionSchemaSpec {
    pub option_schema_id: String,
    #[serde(default)]
    pub fields: Vec<DriverOptionField>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DriverOptionField {
    ApiKeyField,
    CustomUserAgent,
    CodexFastMode,
    CodexImageGenerationEnabled,
    CodexWebsocketEnabled,
}

impl DriverOptionField {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApiKeyField => "apiKeyField",
            Self::CustomUserAgent => "customUserAgent",
            Self::CodexFastMode => "codexFastMode",
            Self::CodexImageGenerationEnabled => "codexImageGenerationEnabled",
            Self::CodexWebsocketEnabled => "codexWebsocketEnabled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderFamilySpec {
    pub family_id: String,
    pub label: String,
    pub credential_profile_id: ProfileId,
    pub endpoint_scope: ProviderFieldScope,
    pub headers_scope: ProviderFieldScope,
    pub driver_options_scope: ProviderFieldScope,
    pub credential_source_scope: CredentialSourceScope,
    pub surfaces: Vec<ProviderFamilySurfaceSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderFamilySurfaceSpec {
    pub app: AppKind,
    pub profile_id: ProfileId,
    pub default_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFieldScope {
    Bundle,
    Surface,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSourceScope {
    Bundle,
    Surface,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileSpec {
    pub profile_id: ProfileId,
    pub profile_schema_revision: u32,
    pub app: AppKind,
    pub label: String,
    pub driver_binding: DriverBinding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility_provider_type: Option<ProviderType>,
    pub form_composition: FormComposition,
    pub endpoint_policy: EndpointPolicy,
    pub credential_policy: CredentialPolicy,
    pub model_policy: ModelPolicyKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_model_policies: Vec<ModelPolicyKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_upstream_model: Option<String>,
    pub visibility: ProfileVisibility,
    pub creation_policy: CreationPolicy,
    pub maturity: ProfileMaturity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DriverBinding {
    Fixed { driver_id: DriverId },
    Custom { custom_policy_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormComposition {
    ManagedAccount,
    StaticSecret,
    Aws,
    Custom,
    Legacy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointPolicy {
    Fixed,
    OverrideAllowed,
    Template,
    Custom,
    FrozenLegacy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CredentialPolicy {
    ManagedAccount {
        account_provider_type: ProviderType,
    },
    StaticSecret {
        slots: Vec<String>,
        auth_scheme: AuthScheme,
    },
    Aws {
        slots: Vec<String>,
    },
    Custom,
    Legacy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPolicyKind {
    Passthrough,
    Single,
}

impl ProfileSpec {
    pub fn allows_model_policy(&self, policy: ModelPolicyKind) -> bool {
        if self.allowed_model_policies.is_empty() {
            self.model_policy == policy
        } else {
            self.allowed_model_policies.contains(&policy)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileVisibility {
    Visible,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreationPolicy {
    CreateAllowed,
    ExistingOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileMaturity {
    Stable,
    Experimental,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DriverSpec {
    pub driver_id: DriverId,
    pub driver_contract_revision: u32,
    pub upstream_protocol: UpstreamProtocol,
    pub accepted_auth_schemes: Vec<AuthScheme>,
    pub operations: DriverOperations,
    pub capabilities: DriverCapabilities,
    pub outbound_identity_policy: OutboundIdentityPolicy,
    pub option_schema_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum OutboundIdentityPolicy {
    ManagedIdentity { family: ManagedIdentityFamily },
    ManagedVersion { family: ManagedVersionFamily },
    ServerIdentity,
    Omit,
    CustomOverride,
    LegacyFrozen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedIdentityFamily {
    ClaudeCode,
    CodexCli,
    GeminiCli,
    GrokCli,
    Kiro,
    Cursor,
    Copilot,
    Deepseek,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedVersionFamily {
    Antigravity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamProtocol {
    AnthropicMessages,
    OpenAiChat,
    OpenAiResponses,
    GeminiNative,
    Bedrock,
    Special,
    Custom,
    Legacy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthScheme {
    None,
    ApiKey,
    Bearer,
    #[serde(rename = "oauth")]
    OAuth,
    AwsSigV4,
    CustomHeader,
    Query,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DriverOperations {
    pub forward: OperationSupport,
    pub test: OperationSupport,
    pub discovery: OperationSupport,
    pub connectivity: OperationSupport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationSupport {
    Supported,
    Unsupported,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DriverCapabilities {
    pub stream: bool,
    pub tools: bool,
    pub images: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomPolicySpec {
    pub custom_policy_id: String,
    pub app: AppKind,
    pub protocols: Vec<UpstreamProtocol>,
    pub auth_schemes: Vec<AuthScheme>,
    pub allowed_driver_ids: Vec<DriverId>,
    pub outbound_identity_policy: OutboundIdentityPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomBindingInput {
    pub upstream_protocol: UpstreamProtocol,
    pub auth_scheme: AuthScheme,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedCustomBinding {
    pub custom_policy_id: String,
    pub driver_id: DriverId,
    pub upstream_protocol: UpstreamProtocol,
    pub auth_scheme: AuthScheme,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyPresetMapping {
    pub app: AppKind,
    pub legacy_name: String,
    pub profile_id: ProfileId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishedIdTombstone {
    pub id: String,
    pub kind: PublishedIdKind,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishedIdKind {
    Profile,
    Driver,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DriverConformance {
    pub driver_id: DriverId,
    pub forward: ConformanceState,
    pub test: ConformanceState,
    pub discovery: ConformanceState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConformanceState {
    Implemented,
    FixtureVerified,
    LivePending,
    Unsupported,
}

static REGISTRY: OnceLock<ProviderRegistry> = OnceLock::new();

pub fn provider_registry() -> &'static ProviderRegistry {
    REGISTRY.get_or_init(|| {
        let registry: ProviderRegistry = serde_json::from_str(include_str!(
            "../../../assets/contract/provider-registry.json"
        ))
        .expect("embedded Provider registry must decode");
        validate_registry(&registry).expect("embedded Provider registry must be valid");
        registry
    })
}

pub fn profile_by_id(profile_id: &str) -> Option<&'static ProfileSpec> {
    provider_registry()
        .profiles
        .iter()
        .find(|profile| profile.profile_id.as_str() == profile_id)
}

pub fn family_by_id(family_id: &str) -> Option<&'static ProviderFamilySpec> {
    provider_registry()
        .families
        .iter()
        .find(|family| family.family_id == family_id)
}

pub fn family_for_profile(profile_id: &str) -> Option<&'static ProviderFamilySpec> {
    provider_registry().families.iter().find(|family| {
        family
            .surfaces
            .iter()
            .any(|surface| surface.profile_id.as_str() == profile_id)
    })
}

pub fn option_schema_by_id(option_schema_id: &str) -> Option<&'static DriverOptionSchemaSpec> {
    provider_registry()
        .option_schemas
        .iter()
        .find(|schema| schema.option_schema_id == option_schema_id)
}

pub fn validate_driver_option_input(
    driver: &DriverSpec,
    configured_fields: &BTreeSet<&'static str>,
) -> anyhow::Result<()> {
    let schema = option_schema_by_id(&driver.option_schema_id).with_context(|| {
        format!(
            "Driver {} references unknown option schema {}",
            driver.driver_id, driver.option_schema_id
        )
    })?;
    let allowed = schema
        .fields
        .iter()
        .map(|field| field.as_str())
        .collect::<BTreeSet<_>>();
    if let Some(field) = configured_fields.difference(&allowed).next() {
        bail!(
            "Driver {} does not allow option {}",
            driver.driver_id,
            field
        );
    }
    Ok(())
}

pub fn profile_for_legacy_preset(app: AppKind, legacy_name: &str) -> Option<&'static ProfileSpec> {
    let mapping = provider_registry()
        .legacy_preset_mappings
        .iter()
        .find(|mapping| mapping.app == app && mapping.legacy_name == legacy_name)?;
    profile_by_id(mapping.profile_id.as_str())
}

pub fn resolve_custom_binding(
    profile: &ProfileSpec,
    input: &CustomBindingInput,
) -> anyhow::Result<ResolvedCustomBinding> {
    let DriverBinding::Custom { custom_policy_id } = &profile.driver_binding else {
        bail!("profile {} is not a custom Profile", profile.profile_id);
    };
    let registry = provider_registry();
    let policy = registry
        .custom_policies
        .iter()
        .find(|policy| policy.custom_policy_id == *custom_policy_id)
        .with_context(|| format!("custom policy {custom_policy_id} is not registered"))?;
    if policy.app != profile.app {
        bail!("custom policy {custom_policy_id} belongs to a different app");
    }
    if !policy.protocols.contains(&input.upstream_protocol) {
        bail!(
            "custom policy {custom_policy_id} does not allow protocol {:?}",
            input.upstream_protocol
        );
    }
    if !policy.auth_schemes.contains(&input.auth_scheme) {
        bail!(
            "custom policy {custom_policy_id} does not allow auth scheme {:?}",
            input.auth_scheme
        );
    }
    let matching = policy
        .allowed_driver_ids
        .iter()
        .filter_map(|driver_id| {
            registry
                .drivers
                .iter()
                .find(|driver| driver.driver_id == *driver_id)
        })
        .filter(|driver| {
            driver.upstream_protocol == input.upstream_protocol
                && driver.accepted_auth_schemes.contains(&input.auth_scheme)
        })
        .collect::<Vec<_>>();
    if matching.len() != 1 {
        bail!(
            "custom binding must resolve to exactly one Driver, resolved {}",
            matching.len()
        );
    }
    Ok(ResolvedCustomBinding {
        custom_policy_id: custom_policy_id.clone(),
        driver_id: matching[0].driver_id.clone(),
        upstream_protocol: input.upstream_protocol,
        auth_scheme: input.auth_scheme,
    })
}

pub fn validate_registry(registry: &ProviderRegistry) -> anyhow::Result<()> {
    if registry.format != PROVIDER_REGISTRY_FORMAT {
        bail!("unexpected Provider registry format {}", registry.format);
    }
    if registry.schema_version != PROVIDER_REGISTRY_SCHEMA_VERSION {
        bail!(
            "unsupported Provider registry schema version {}",
            registry.schema_version
        );
    }

    let mut profile_ids = BTreeSet::new();
    let mut driver_ids = BTreeSet::new();
    let mut custom_policy_ids = BTreeSet::new();
    let mut option_schema_ids = BTreeSet::new();
    for schema in &registry.option_schemas {
        validate_registry_id(&schema.option_schema_id, "Driver option schema")?;
        if !option_schema_ids.insert(schema.option_schema_id.as_str()) {
            bail!(
                "duplicate Driver option schema id {}",
                schema.option_schema_id
            );
        }
        if schema.fields.iter().collect::<BTreeSet<_>>().len() != schema.fields.len() {
            bail!(
                "Driver option schema {} repeats a field",
                schema.option_schema_id
            );
        }
    }
    for driver in &registry.drivers {
        validate_registry_id(driver.driver_id.as_str(), "driver")?;
        if driver.driver_contract_revision == 0 {
            bail!("driver {} has revision zero", driver.driver_id);
        }
        if !driver_ids.insert(driver.driver_id.as_str()) {
            bail!("duplicate driver id {}", driver.driver_id);
        }
        if !option_schema_ids.contains(driver.option_schema_id.as_str()) {
            bail!(
                "driver {} references unknown option schema {}",
                driver.driver_id,
                driver.option_schema_id
            );
        }
        if driver.outbound_identity_policy == OutboundIdentityPolicy::CustomOverride {
            bail!(
                "driver {} cannot delegate outbound identity to a Provider override",
                driver.driver_id
            );
        }
        validate_operation_contract(driver)?;
    }
    let referenced_option_schema_ids = registry
        .drivers
        .iter()
        .map(|driver| driver.option_schema_id.as_str())
        .collect::<BTreeSet<_>>();
    if referenced_option_schema_ids != option_schema_ids {
        bail!("every Driver option schema must be referenced by at least one Driver");
    }
    for policy in &registry.custom_policies {
        validate_registry_id(&policy.custom_policy_id, "custom policy")?;
        if !custom_policy_ids.insert(policy.custom_policy_id.as_str()) {
            bail!("duplicate custom policy id {}", policy.custom_policy_id);
        }
        if policy.allowed_driver_ids.is_empty() {
            bail!(
                "custom policy {} has no allowed drivers",
                policy.custom_policy_id
            );
        }
        if policy.protocols.is_empty() || policy.auth_schemes.is_empty() {
            bail!(
                "custom policy {} must declare protocols and auth schemes",
                policy.custom_policy_id
            );
        }
        if policy.outbound_identity_policy != OutboundIdentityPolicy::CustomOverride {
            bail!(
                "custom policy {} must use custom_override outbound identity",
                policy.custom_policy_id
            );
        }
        if policy
            .protocols
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .len()
            != policy.protocols.len()
            || policy
                .auth_schemes
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
                .len()
                != policy.auth_schemes.len()
        {
            bail!(
                "custom policy {} repeats a protocol or auth scheme",
                policy.custom_policy_id
            );
        }
        for driver_id in &policy.allowed_driver_ids {
            if !driver_ids.contains(driver_id.as_str()) {
                bail!(
                    "custom policy {} references unknown driver {}",
                    policy.custom_policy_id,
                    driver_id
                );
            }
        }
    }
    for profile in &registry.profiles {
        validate_registry_id(profile.profile_id.as_str(), "profile")?;
        if !profile
            .profile_id
            .as_str()
            .starts_with(profile.app.as_str())
            || !profile.profile_id.as_str()[profile.app.as_str().len()..].starts_with('.')
        {
            bail!(
                "profile {} is not namespaced for {}",
                profile.profile_id,
                profile.app.as_str()
            );
        }
        if profile.profile_schema_revision == 0 {
            bail!("profile {} has revision zero", profile.profile_id);
        }
        if profile.label.trim().is_empty() || profile.label != profile.label.trim() {
            bail!("profile {} has an invalid label", profile.profile_id);
        }
        if !profile_ids.insert(profile.profile_id.as_str()) {
            bail!("duplicate profile id {}", profile.profile_id);
        }
        validate_profile_contract(
            profile,
            &registry.drivers,
            &registry.custom_policies,
            &driver_ids,
            &custom_policy_ids,
        )?;
    }

    let creatable_profile_ids = registry
        .profiles
        .iter()
        .filter(|profile| {
            profile.visibility == ProfileVisibility::Visible
                && profile.creation_policy == CreationPolicy::CreateAllowed
        })
        .map(|profile| profile.profile_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut family_ids = BTreeSet::new();
    let mut family_profile_ids = BTreeSet::new();
    for family in &registry.families {
        validate_registry_id(&family.family_id, "family")?;
        if !family_ids.insert(family.family_id.as_str()) {
            bail!("duplicate Provider family id {}", family.family_id);
        }
        if family.label.trim().is_empty() || family.label != family.label.trim() {
            bail!("Provider family {} has an invalid label", family.family_id);
        }
        if family.surfaces.is_empty() || family.surfaces.len() > 3 {
            bail!(
                "Provider family {} must contain between one and three surfaces",
                family.family_id
            );
        }
        let credential_profile = registry
            .profiles
            .iter()
            .find(|profile| profile.profile_id == family.credential_profile_id)
            .with_context(|| {
                format!(
                    "Provider family {} references unknown credential profile {}",
                    family.family_id, family.credential_profile_id
                )
            })?;
        if family.credential_source_scope == CredentialSourceScope::Bundle
            && family.surfaces.len() > 1
            && matches!(
                credential_profile.credential_policy,
                CredentialPolicy::Custom | CredentialPolicy::Legacy
            )
        {
            bail!(
                "Provider family {} cannot share Surface-scoped credentials at Bundle scope",
                family.family_id
            );
        }
        let mut apps = BTreeSet::new();
        for surface in &family.surfaces {
            if !apps.insert(surface.app) {
                bail!(
                    "Provider family {} repeats the {} surface",
                    family.family_id,
                    surface.app.as_str()
                );
            }
            let profile = registry
                .profiles
                .iter()
                .find(|profile| profile.profile_id == surface.profile_id)
                .with_context(|| {
                    format!(
                        "Provider family {} references unknown profile {}",
                        family.family_id, surface.profile_id
                    )
                })?;
            if profile.app != surface.app {
                bail!(
                    "Provider family {} surface {} references a profile from another app",
                    family.family_id,
                    surface.app.as_str()
                );
            }
            if profile.visibility != ProfileVisibility::Visible
                || profile.creation_policy != CreationPolicy::CreateAllowed
            {
                bail!(
                    "Provider family {} references non-creatable profile {}",
                    family.family_id,
                    surface.profile_id
                );
            }
            if profile.credential_policy != credential_profile.credential_policy {
                bail!(
                    "Provider family {} surfaces do not share one credential policy",
                    family.family_id
                );
            }
            if !family_profile_ids.insert(surface.profile_id.as_str()) {
                bail!(
                    "Provider profile {} belongs to more than one family",
                    surface.profile_id
                );
            }
        }
    }
    if family_profile_ids != creatable_profile_ids {
        let missing = creatable_profile_ids
            .difference(&family_profile_ids)
            .copied()
            .collect::<Vec<_>>();
        let unexpected = family_profile_ids
            .difference(&creatable_profile_ids)
            .copied()
            .collect::<Vec<_>>();
        bail!(
            "Provider family coverage disagrees with creatable profiles; missing={missing:?}, unexpected={unexpected:?}"
        );
    }

    let expected_counts = BTreeMap::from([
        (AppKind::Claude, 19usize),
        (AppKind::Codex, 9usize),
        (AppKind::Gemini, 6usize),
    ]);
    for (app, expected) in expected_counts {
        let actual = registry
            .profiles
            .iter()
            .filter(|profile| {
                profile.app == app
                    && !matches!(
                        profile.form_composition,
                        FormComposition::Custom | FormComposition::Legacy
                    )
            })
            .count();
        if actual != expected {
            bail!(
                "{} first-class profile count is {actual}, expected {expected}",
                app.as_str()
            );
        }
    }
    if registry.profiles.len() != 40 {
        bail!(
            "Provider registry contains {} profiles, expected 40",
            registry.profiles.len()
        );
    }
    if registry.legacy_preset_mappings.len() != 29 {
        bail!(
            "Provider registry contains {} legacy preset mappings, expected 29",
            registry.legacy_preset_mappings.len()
        );
    }

    let mut mappings = BTreeSet::new();
    let mut mapped_profile_ids = BTreeSet::new();
    for mapping in &registry.legacy_preset_mappings {
        if !mappings.insert((mapping.app, mapping.legacy_name.as_str())) {
            bail!(
                "duplicate legacy preset mapping {}:{}",
                mapping.app.as_str(),
                mapping.legacy_name
            );
        }
        let profile = registry
            .profiles
            .iter()
            .find(|profile| profile.profile_id == mapping.profile_id)
            .with_context(|| {
                format!(
                    "legacy preset mapping references unknown profile {}",
                    mapping.profile_id
                )
            })?;
        if profile.app != mapping.app {
            bail!(
                "legacy preset mapping {}:{} crosses app boundary",
                mapping.app.as_str(),
                mapping.legacy_name
            );
        }
        if !mapped_profile_ids.insert(mapping.profile_id.as_str()) {
            bail!(
                "profile {} is mapped from more than one legacy preset",
                mapping.profile_id
            );
        }
    }

    let reviewed_first_class_additions = BTreeSet::from([
        "claude.anthropic_api_key",
        "claude.bearer_relay",
        "claude.google_oauth",
        "codex.openai_api_key",
        "gemini.google_api_key",
    ]);
    let expected_mapped_profile_ids = registry
        .profiles
        .iter()
        .filter(|profile| {
            !matches!(
                profile.form_composition,
                FormComposition::Custom | FormComposition::Legacy
            ) && !reviewed_first_class_additions.contains(profile.profile_id.as_str())
        })
        .map(|profile| profile.profile_id.as_str())
        .collect::<BTreeSet<_>>();
    if mapped_profile_ids != expected_mapped_profile_ids {
        let missing = expected_mapped_profile_ids
            .difference(&mapped_profile_ids)
            .copied()
            .collect::<Vec<_>>();
        let unexpected = mapped_profile_ids
            .difference(&expected_mapped_profile_ids)
            .copied()
            .collect::<Vec<_>>();
        bail!(
            "legacy preset mappings disagree with historical Profiles; missing={missing:?}, unexpected={unexpected:?}"
        );
    }

    let tombstones = registry
        .published_id_tombstones
        .iter()
        .map(|tombstone| tombstone.id.as_str())
        .collect::<BTreeSet<_>>();
    if tombstones.len() != registry.published_id_tombstones.len() {
        bail!("duplicate published id tombstone");
    }
    if profile_ids.iter().any(|id| tombstones.contains(id))
        || driver_ids.iter().any(|id| tombstones.contains(id))
    {
        bail!("active Provider registry id reuses a tombstone");
    }
    for tombstone in &registry.published_id_tombstones {
        validate_registry_id(&tombstone.id, "tombstone")?;
        if tombstone.reason.trim().is_empty() {
            bail!("published id tombstone {} has no reason", tombstone.id);
        }
    }

    let conformance = registry
        .conformance
        .iter()
        .map(|item| item.driver_id.as_str())
        .collect::<BTreeSet<_>>();
    if conformance.len() != registry.conformance.len() || conformance != driver_ids {
        bail!("conformance matrix must contain each driver exactly once");
    }
    for item in &registry.conformance {
        let driver = registry
            .drivers
            .iter()
            .find(|driver| driver.driver_id == item.driver_id)
            .expect("conformance driver set was checked");
        validate_conformance_state(driver, "forward", driver.operations.forward, item.forward)?;
        validate_conformance_state(driver, "test", driver.operations.test, item.test)?;
        validate_conformance_state(
            driver,
            "discovery",
            driver.operations.discovery,
            item.discovery,
        )?;
    }
    Ok(())
}

fn validate_profile_contract(
    profile: &ProfileSpec,
    drivers: &[DriverSpec],
    custom_policies: &[CustomPolicySpec],
    driver_ids: &BTreeSet<&str>,
    custom_policy_ids: &BTreeSet<&str>,
) -> anyhow::Result<()> {
    let allowed_model_policies = if profile.allowed_model_policies.is_empty() {
        vec![profile.model_policy]
    } else {
        profile.allowed_model_policies.clone()
    };
    if !allowed_model_policies.contains(&profile.model_policy) {
        bail!(
            "profile {} default model policy is not allowed",
            profile.profile_id
        );
    }
    if allowed_model_policies.iter().collect::<BTreeSet<_>>().len() != allowed_model_policies.len()
    {
        bail!(
            "profile {} declares duplicate allowed model policies",
            profile.profile_id
        );
    }
    let default_upstream_model = profile
        .default_upstream_model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty());
    if profile.default_upstream_model.is_some() && default_upstream_model.is_none() {
        bail!(
            "profile {} declares an empty default upstream model",
            profile.profile_id
        );
    }
    if profile.form_composition == FormComposition::Legacy
        && (!profile.allowed_model_policies.is_empty() || profile.default_upstream_model.is_some())
    {
        bail!(
            "legacy profile {} cannot declare configurable model policy fields",
            profile.profile_id
        );
    }
    match profile.model_policy {
        ModelPolicyKind::Single => {
            if default_upstream_model.is_none()
                && profile.creation_policy == CreationPolicy::CreateAllowed
            {
                bail!(
                    "single-model profile {} has no default upstream model",
                    profile.profile_id
                );
            }
        }
        ModelPolicyKind::Passthrough if profile.default_upstream_model.is_some() => {
            bail!(
                "passthrough profile {} cannot declare a default upstream model",
                profile.profile_id
            );
        }
        ModelPolicyKind::Passthrough => {}
    }

    match (&profile.form_composition, &profile.credential_policy) {
        (FormComposition::ManagedAccount, CredentialPolicy::ManagedAccount { .. })
        | (FormComposition::StaticSecret, CredentialPolicy::StaticSecret { .. })
        | (FormComposition::Aws, CredentialPolicy::Aws { .. })
        | (FormComposition::Custom, CredentialPolicy::Custom)
        | (FormComposition::Legacy, CredentialPolicy::Legacy) => {}
        _ => bail!(
            "profile {} form and credential policies disagree",
            profile.profile_id
        ),
    }

    match &profile.driver_binding {
        DriverBinding::Fixed { driver_id } => {
            if !driver_ids.contains(driver_id.as_str()) {
                bail!(
                    "profile {} references unknown driver {}",
                    profile.profile_id,
                    driver_id
                );
            }
            if profile.form_composition == FormComposition::Custom {
                bail!("custom profile {} has a fixed driver", profile.profile_id);
            }
            let driver = drivers
                .iter()
                .find(|driver| driver.driver_id == *driver_id)
                .expect("driver id set was checked");
            if profile.form_composition == FormComposition::Legacy
                && driver.outbound_identity_policy != OutboundIdentityPolicy::LegacyFrozen
            {
                bail!(
                    "legacy profile {} must use a legacy_frozen identity driver",
                    profile.profile_id
                );
            }
            if profile.form_composition != FormComposition::Legacy
                && driver.outbound_identity_policy == OutboundIdentityPolicy::LegacyFrozen
            {
                bail!(
                    "profile {} cannot use a legacy_frozen identity driver",
                    profile.profile_id
                );
            }
            match &profile.credential_policy {
                CredentialPolicy::ManagedAccount { .. }
                    if !driver.accepted_auth_schemes.contains(&AuthScheme::OAuth) =>
                {
                    bail!(
                        "managed profile {} uses a driver without OAuth",
                        profile.profile_id
                    );
                }
                CredentialPolicy::StaticSecret { slots, auth_scheme } => {
                    if slots.is_empty() {
                        bail!(
                            "static profile {} declares no secret slots",
                            profile.profile_id
                        );
                    }
                    if !driver.accepted_auth_schemes.contains(auth_scheme) {
                        bail!(
                            "static profile {} uses auth scheme {:?}, which driver {} does not accept",
                            profile.profile_id,
                            auth_scheme,
                            driver.driver_id
                        );
                    }
                }
                CredentialPolicy::Aws { slots }
                    if slots.is_empty()
                        || !driver.accepted_auth_schemes.contains(&AuthScheme::AwsSigV4) =>
                {
                    bail!(
                        "AWS profile {} has an invalid credential contract",
                        profile.profile_id
                    );
                }
                _ => {}
            }
        }
        DriverBinding::Custom { custom_policy_id } => {
            if !custom_policy_ids.contains(custom_policy_id.as_str()) {
                bail!(
                    "profile {} references unknown custom policy {}",
                    profile.profile_id,
                    custom_policy_id
                );
            }
            let policy = custom_policies
                .iter()
                .find(|policy| policy.custom_policy_id == *custom_policy_id)
                .expect("custom policy id set was checked");
            if profile.form_composition != FormComposition::Custom || policy.app != profile.app {
                bail!(
                    "custom profile {} and policy {} disagree",
                    profile.profile_id,
                    custom_policy_id
                );
            }
        }
    }

    if profile.form_composition == FormComposition::Legacy
        && profile.creation_policy != CreationPolicy::ExistingOnly
    {
        bail!("legacy profile {} allows creation", profile.profile_id);
    }
    Ok(())
}

fn validate_conformance_state(
    driver: &DriverSpec,
    operation: &str,
    support: OperationSupport,
    state: ConformanceState,
) -> anyhow::Result<()> {
    let valid = match support {
        OperationSupport::Unsupported => state == ConformanceState::Unsupported,
        OperationSupport::Supported => state != ConformanceState::Unsupported,
    };
    if !valid {
        bail!(
            "driver {} {operation} support and conformance disagree",
            driver.driver_id
        );
    }
    Ok(())
}

fn validate_registry_id(value: &str, kind: &str) -> anyhow::Result<()> {
    let valid = value.len() >= 3
        && value.len() <= 96
        && value == value.trim()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
        && value.bytes().any(|byte| byte == b'.');
    if !valid {
        bail!("invalid {kind} id {value:?}");
    }
    Ok(())
}

fn validate_operation_contract(driver: &DriverSpec) -> anyhow::Result<()> {
    if driver.operations.forward == OperationSupport::Unsupported {
        bail!("driver {} cannot omit forward support", driver.driver_id);
    }
    if driver.option_schema_id.trim().is_empty() {
        bail!("driver {} has an empty option schema", driver.driver_id);
    }
    let auth = driver
        .accepted_auth_schemes
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if auth.len() != driver.accepted_auth_schemes.len() {
        bail!("driver {} repeats an auth scheme", driver.driver_id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_registry_is_valid_and_has_expected_inventory() {
        let registry = provider_registry();
        validate_registry(registry).unwrap();

        assert_eq!(registry.profiles.len(), 40);
        assert_eq!(registry.legacy_preset_mappings.len(), 29);
        assert_eq!(
            registry
                .profiles
                .iter()
                .filter(|profile| profile.form_composition == FormComposition::Custom)
                .count(),
            3
        );
        assert_eq!(
            registry
                .profiles
                .iter()
                .filter(|profile| profile.form_composition == FormComposition::Legacy)
                .count(),
            3
        );
    }

    #[test]
    fn legacy_mappings_resolve_to_the_declared_app() {
        for mapping in &provider_registry().legacy_preset_mappings {
            let profile = profile_for_legacy_preset(mapping.app, &mapping.legacy_name).unwrap();
            assert_eq!(profile.app, mapping.app);
            assert_eq!(profile.profile_id, mapping.profile_id);
        }
    }

    #[test]
    fn required_provider_type_app_pairs_have_a_creatable_visible_profile() {
        let required = [
            (ProviderType::Claude, AppKind::Claude),
            (ProviderType::ClaudeAuth, AppKind::Claude),
            (ProviderType::ClaudeOAuth, AppKind::Claude),
            (ProviderType::Codex, AppKind::Codex),
            (ProviderType::CodexOAuth, AppKind::Claude),
            (ProviderType::CodexOAuth, AppKind::Codex),
            (ProviderType::Gemini, AppKind::Gemini),
            (ProviderType::GeminiCli, AppKind::Claude),
            (ProviderType::GeminiCli, AppKind::Gemini),
            (ProviderType::OpenRouter, AppKind::Claude),
            (ProviderType::OpenRouter, AppKind::Codex),
            (ProviderType::OpenRouter, AppKind::Gemini),
            (ProviderType::GitHubCopilot, AppKind::Claude),
            (ProviderType::DeepSeekAccount, AppKind::Claude),
            (ProviderType::KiroOAuth, AppKind::Claude),
            (ProviderType::CursorOAuth, AppKind::Claude),
            (ProviderType::CursorOAuth, AppKind::Codex),
            (ProviderType::CursorApiKey, AppKind::Claude),
            (ProviderType::CursorApiKey, AppKind::Codex),
            (ProviderType::AntigravityOAuth, AppKind::Claude),
            (ProviderType::AntigravityOAuth, AppKind::Gemini),
            (ProviderType::AgyOAuth, AppKind::Claude),
            (ProviderType::AgyOAuth, AppKind::Gemini),
            (ProviderType::OllamaCloud, AppKind::Claude),
            (ProviderType::OllamaCloud, AppKind::Codex),
            (ProviderType::AwsBedrock, AppKind::Claude),
            (ProviderType::Nvidia, AppKind::Claude),
            (ProviderType::Nvidia, AppKind::Codex),
            (ProviderType::DeepSeekApi, AppKind::Claude),
            (ProviderType::DeepSeekApi, AppKind::Codex),
            (ProviderType::GrokOAuth, AppKind::Claude),
            (ProviderType::GrokOAuth, AppKind::Codex),
            (ProviderType::GrokOAuth, AppKind::Gemini),
        ];

        for (provider_type, app) in required {
            let profiles = provider_registry()
                .profiles
                .iter()
                .filter(|profile| {
                    profile.app == app
                        && profile.compatibility_provider_type == Some(provider_type)
                        && profile.visibility == ProfileVisibility::Visible
                        && profile.creation_policy == CreationPolicy::CreateAllowed
                })
                .collect::<Vec<_>>();
            assert!(
                !profiles.is_empty(),
                "missing visible create_allowed Profile for {}:{}",
                app.as_str(),
                provider_type.as_str()
            );
            for profile in profiles {
                if let CredentialPolicy::ManagedAccount {
                    account_provider_type,
                } = &profile.credential_policy
                {
                    assert_eq!(
                        *account_provider_type, provider_type,
                        "{}",
                        profile.profile_id
                    );
                }
            }
        }
    }

    #[test]
    fn model_policy_capabilities_lock_native_profiles_and_configure_others() {
        for profile in &provider_registry().profiles {
            if profile.form_composition == FormComposition::Legacy {
                assert!(profile.allowed_model_policies.is_empty());
                assert!(profile.default_upstream_model.is_none());
                continue;
            }
            match profile.model_policy {
                ModelPolicyKind::Passthrough => {
                    assert!(profile.allows_model_policy(ModelPolicyKind::Passthrough));
                    assert!(!profile.allows_model_policy(ModelPolicyKind::Single));
                    assert!(profile.default_upstream_model.is_none());
                }
                ModelPolicyKind::Single => {
                    assert!(profile.allows_model_policy(ModelPolicyKind::Single));
                    assert!(profile.allows_model_policy(ModelPolicyKind::Passthrough));
                    assert!(profile
                        .default_upstream_model
                        .as_deref()
                        .is_some_and(|model| !model.trim().is_empty()));
                }
            }
        }
    }

    #[test]
    fn provider_key_requires_an_explicit_app_and_trimmed_id() {
        assert_eq!(
            ProviderKey::new(AppKind::Claude, "same-id").unwrap().app,
            AppKind::Claude
        );
        assert!(ProviderKey::new(AppKind::Codex, "").is_err());
        assert!(ProviderKey::new(AppKind::Gemini, " id ").is_err());
    }

    #[test]
    fn outbound_identity_is_explicit_for_every_driver_and_custom_policy() {
        let registry = provider_registry();
        assert!(registry.drivers.iter().all(|driver| {
            driver.outbound_identity_policy != OutboundIdentityPolicy::CustomOverride
        }));
        assert!(registry.custom_policies.iter().all(|policy| {
            policy.outbound_identity_policy == OutboundIdentityPolicy::CustomOverride
        }));
        assert_eq!(
            registry
                .drivers
                .iter()
                .find(|driver| driver.driver_id.as_str() == "oauth.openai_codex")
                .unwrap()
                .outbound_identity_policy,
            OutboundIdentityPolicy::ManagedIdentity {
                family: ManagedIdentityFamily::CodexCli
            }
        );
        assert_eq!(
            registry
                .drivers
                .iter()
                .find(|driver| driver.driver_id.as_str() == "oauth.gemini_code_assist")
                .unwrap()
                .outbound_identity_policy,
            OutboundIdentityPolicy::ManagedIdentity {
                family: ManagedIdentityFamily::GeminiCli
            }
        );
        assert_eq!(
            registry
                .drivers
                .iter()
                .find(|driver| driver.driver_id.as_str() == "aws.bedrock_sigv4")
                .unwrap()
                .outbound_identity_policy,
            OutboundIdentityPolicy::Omit
        );
    }
}
