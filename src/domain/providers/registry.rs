use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};

use super::model::{AppKind, ProviderType};

pub const PROVIDER_REGISTRY_SCHEMA_VERSION: u32 = 2;
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
    pub profiles: Vec<ProfileSpec>,
    pub drivers: Vec<DriverSpec>,
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
    pub visibility: ProfileVisibility,
    pub creation_policy: CreationPolicy,
    pub maturity: ProfileMaturity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPolicyKind {
    Passthrough,
    Single,
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
    for driver in &registry.drivers {
        validate_registry_id(driver.driver_id.as_str(), "driver")?;
        if driver.driver_contract_revision == 0 {
            bail!("driver {} has revision zero", driver.driver_id);
        }
        if !driver_ids.insert(driver.driver_id.as_str()) {
            bail!("duplicate driver id {}", driver.driver_id);
        }
        if driver.outbound_identity_policy == OutboundIdentityPolicy::CustomOverride {
            bail!(
                "driver {} cannot delegate outbound identity to a Provider override",
                driver.driver_id
            );
        }
        validate_operation_contract(driver)?;
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

    let expected_counts = BTreeMap::from([
        (AppKind::Claude, 17usize),
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
    if registry.profiles.len() != 38 {
        bail!(
            "Provider registry contains {} profiles, expected 38",
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

    let direct_profile_ids = BTreeSet::from([
        "claude.anthropic_api_key",
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
            ) && !direct_profile_ids.contains(profile.profile_id.as_str())
        })
        .map(|profile| profile.profile_id.as_str())
        .collect::<BTreeSet<_>>();
    if mapped_profile_ids != expected_mapped_profile_ids {
        bail!("legacy preset mappings do not cover exactly the 29 historical Profiles");
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

        assert_eq!(registry.profiles.len(), 38);
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
                .find(|driver| driver.driver_id.as_str() == "aws.bedrock_sigv4")
                .unwrap()
                .outbound_identity_policy,
            OutboundIdentityPolicy::Omit
        );
    }
}
