use std::collections::{BTreeMap, BTreeSet};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroize;

use super::model::Provider;
use super::registry::{
    profile_by_id, profile_for_legacy_preset, CustomBindingInput, ProfileId, ProviderKey,
};
use super::runtime::RuntimeConfigurationState;
use super::store::StoredProvider;

pub const SECRET_KEEP_SENTINEL: &str = "__CC_SWITCH_SECRET_KEEP__";

const KNOWN_JSON_SECRET_POINTERS: &[&str] = &[
    "/settingsConfig/auth/OPENAI_API_KEY",
    "/settingsConfig/apiKey",
    "/settingsConfig/env/ANTHROPIC_API_KEY",
    "/settingsConfig/env/ANTHROPIC_AUTH_TOKEN",
    "/settingsConfig/env/API_KEY",
    "/settingsConfig/env/AWS_ACCESS_KEY_ID",
    "/settingsConfig/env/AWS_SECRET_ACCESS_KEY",
    "/settingsConfig/env/AWS_SESSION_TOKEN",
    "/settingsConfig/env/CODEX_API_KEY",
    "/settingsConfig/env/GEMINI_API_KEY",
    "/settingsConfig/env/GOOGLE_API_KEY",
    "/settingsConfig/env/GROK_API_KEY",
    "/settingsConfig/env/OPENAI_API_KEY",
    "/settingsConfig/env/XAI_API_KEY",
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialSummary {
    pub configured: bool,
    pub slots: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialPatch {
    Keep,
    Replace { value: String },
    Clear,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderWriteDraft {
    pub app: super::model::AppKind,
    pub provider: Provider,
    pub profile_id: Option<ProfileId>,
    pub custom_binding: Option<CustomBindingInput>,
    pub expected_revision: Option<u64>,
    pub client_request_id: Option<String>,
    pub credential_patches: BTreeMap<String, CredentialPatch>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderCommandError {
    #[error("provider not found")]
    NotFound,
    #[error("{0}")]
    Invalid(String),
    #[error("{message}")]
    Conflict { code: &'static str, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderImportAction {
    Create,
    Update,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderImportItemPreview {
    pub app: super::model::AppKind,
    pub provider_id: String,
    pub name: String,
    pub action: ProviderImportAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderImportPreview {
    pub preview_token: String,
    pub create_count: usize,
    pub update_count: usize,
    pub unchanged_count: usize,
    pub items: Vec<ProviderImportItemPreview>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderReferencePreview {
    pub app: super::model::AppKind,
    pub provider_id: String,
    pub revision: u64,
    pub share_ids: Vec<String>,
    pub current_provider: bool,
    pub blocked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderIdentityAction {
    AdoptProfile,
    RebindCustom,
    CloneAsCustom,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRuntimeTransitionPreview {
    pub before_fingerprint: String,
    pub after_fingerprint: String,
    pub fingerprint_changed: bool,
    pub before_state: RuntimeConfigurationState,
    pub after_state: RuntimeConfigurationState,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderIdentityChangePreview {
    pub preview_token: String,
    pub action: ProviderIdentityAction,
    pub source: ProviderKey,
    pub source_revision: u64,
    pub target: ProviderView,
    pub runtime: ProviderRuntimeTransitionPreview,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAccountBindingMigrationStatus {
    Bound,
    Bindable,
    Ambiguous,
    MissingAccount,
    InvalidAccount,
    StaleIdentity,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAccountBindingMigrationItem {
    pub provider_key: ProviderKey,
    pub provider_revision: u64,
    pub expected_provider_type: super::model::ProviderType,
    pub status: ProviderAccountBindingMigrationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matching_account_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAccountBindingMigrationPreview {
    pub preview_token: String,
    pub bindable_count: usize,
    pub attention_count: usize,
    pub items: Vec<ProviderAccountBindingMigrationItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    pub app: super::model::AppKind,
    pub provider: Provider,
    pub provider_type: super::model::ProviderType,
    pub provider_type_id: String,
    pub revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<ProfileId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_schema_revision: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_binding: Option<CustomBindingInput>,
    pub identity: ProviderIdentityView,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_index: Option<usize>,
    pub credential_configured: bool,
    pub credential_slots: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderIdentityStatus {
    Bound,
    ProfileUpgradeAvailable,
    AdoptionAvailable,
    LegacyCompat,
    NeedsAttention,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderIdentityView {
    pub status: ProviderIdentityStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_profile_id: Option<ProfileId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_profile_schema_revision: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<&'static str>,
}

impl ProviderView {
    pub fn from_stored(stored: &StoredProvider) -> Self {
        Self::from_stored_with_order(stored, None)
    }

    pub fn from_stored_with_order(stored: &StoredProvider, order_index: Option<usize>) -> Self {
        let (provider, credentials) = redact_provider(&stored.provider);
        Self {
            app: stored.app,
            provider,
            provider_type: stored.provider_type,
            provider_type_id: stored.provider_type_id.clone(),
            revision: stored.resource.revision,
            profile_id: stored.resource.profile_id.clone(),
            profile_schema_revision: stored.resource.profile_schema_revision,
            custom_binding: stored.resource.custom_binding.clone(),
            identity: provider_identity_view(stored),
            order_index,
            credential_configured: credentials.configured,
            credential_slots: credentials.slots,
        }
    }
}

fn provider_identity_view(stored: &StoredProvider) -> ProviderIdentityView {
    if let Some(profile_id) = stored.resource.profile_id.as_ref() {
        let Some(profile) = profile_by_id(profile_id.as_str()) else {
            return ProviderIdentityView {
                status: ProviderIdentityStatus::NeedsAttention,
                suggested_profile_id: None,
                current_profile_schema_revision: None,
                warning: Some("stored profileId is not present in this Server registry"),
            };
        };
        if profile.app != stored.app {
            return ProviderIdentityView {
                status: ProviderIdentityStatus::NeedsAttention,
                suggested_profile_id: None,
                current_profile_schema_revision: Some(profile.profile_schema_revision),
                warning: Some("stored profileId belongs to a different app"),
            };
        }
        let stored_revision = stored.resource.profile_schema_revision.unwrap_or_default();
        let status = if stored_revision == profile.profile_schema_revision {
            ProviderIdentityStatus::Bound
        } else if stored_revision < profile.profile_schema_revision {
            ProviderIdentityStatus::ProfileUpgradeAvailable
        } else {
            ProviderIdentityStatus::NeedsAttention
        };
        return ProviderIdentityView {
            status,
            suggested_profile_id: None,
            current_profile_schema_revision: Some(profile.profile_schema_revision),
            warning: (status == ProviderIdentityStatus::NeedsAttention)
                .then_some("stored profile schema revision is newer than this Server understands"),
        };
    }

    if let Some(profile) = profile_for_legacy_preset(stored.app, &stored.provider.name) {
        return ProviderIdentityView {
            status: ProviderIdentityStatus::AdoptionAvailable,
            suggested_profile_id: Some(profile.profile_id.clone()),
            current_profile_schema_revision: Some(profile.profile_schema_revision),
            warning: Some(
                "legacy name matching is only an adoption hint; review before binding this profile",
            ),
        };
    }

    ProviderIdentityView {
        status: ProviderIdentityStatus::LegacyCompat,
        suggested_profile_id: None,
        current_profile_schema_revision: None,
        warning: Some("legacy Provider has no stable profile identity"),
    }
}

pub fn redact_provider(provider: &Provider) -> (Provider, CredentialSummary) {
    let mut value = serde_json::to_value(provider).expect("Provider serialization cannot fail");
    let mut slots = BTreeSet::new();

    for pointer in KNOWN_JSON_SECRET_POINTERS {
        redact_json_pointer(&mut value, pointer, &mut slots);
    }
    redact_dynamic_env(&mut value, &mut slots);
    redact_unknown_secret_keys(&mut value, "", &mut slots);
    redact_toml_credentials(&mut value, &mut slots);

    let provider = serde_json::from_value(value).expect("redacted Provider must remain valid");
    let summary = CredentialSummary {
        configured: !slots.is_empty(),
        slots: slots.into_iter().collect(),
    };
    (provider, summary)
}

pub fn reveal_provider_credential(provider: &Provider, slot: &str) -> anyhow::Result<String> {
    let (_, summary) = redact_provider(provider);
    if !summary.slots.iter().any(|configured| configured == slot) {
        anyhow::bail!("Provider credential slot is not configured: {slot}");
    }

    let value = serde_json::to_value(provider).context("serialize Provider credential source")?;
    credential_value_at_slot(&value, slot)?
        .as_str()
        .map(ToOwned::to_owned)
        .with_context(|| format!("Provider credential slot is not a string: {slot}"))
}

/// Splits a Provider into a committed redacted record and its credential slots.
/// The returned slot values are still plaintext and must only be held while sealing
/// or materializing one operation.
pub fn split_provider_credentials(
    provider: &Provider,
) -> anyhow::Result<(Provider, BTreeMap<String, Value>)> {
    reject_reserved_keep_sentinel(provider)?;
    let original = serde_json::to_value(provider).context("serialize Provider credentials")?;
    let (redacted, summary) = redact_provider(provider);
    let mut credentials = BTreeMap::new();
    for slot in summary.slots {
        let value = credential_value_at_slot(&original, &slot)
            .with_context(|| format!("read Provider credential slot {slot}"))?;
        if !value_is_configured(&value) {
            anyhow::bail!("Provider credential slot {slot} is empty");
        }
        credentials.insert(slot, value);
    }
    Ok((redacted, credentials))
}

pub fn materialize_provider_credentials(
    redacted: &Provider,
    credentials: &BTreeMap<String, Value>,
) -> anyhow::Result<Provider> {
    let (_, summary) = redact_provider(redacted);
    let expected = summary.slots.into_iter().collect::<BTreeSet<_>>();
    let actual = credentials.keys().cloned().collect::<BTreeSet<_>>();
    if expected != actual {
        anyhow::bail!(
            "Provider credential slot mismatch: expected {:?}, received {:?}",
            expected,
            actual
        );
    }

    let mut value = serde_json::to_value(redacted).context("serialize redacted Provider")?;
    for (slot, secret) in credentials {
        set_credential_value_at_slot(&mut value, slot, secret.clone())
            .with_context(|| format!("materialize Provider credential slot {slot}"))?;
    }
    reject_unresolved_sentinels(&value)?;
    let provider: Provider =
        serde_json::from_value(value).context("decode materialized Provider credentials")?;
    let (_, materialized_summary) = redact_provider(&provider);
    if materialized_summary
        .slots
        .into_iter()
        .collect::<BTreeSet<_>>()
        != actual
    {
        anyhow::bail!("materialized Provider credential shape changed");
    }
    Ok(provider)
}

pub fn provider_credential_slot_is_supported(slot: &str) -> bool {
    slot == "/settingsConfig/config"
        || parse_toml_credential_slot(slot).ok().flatten().is_some()
        || is_allowed_json_credential_pointer(slot)
}

pub fn zeroize_materialized_provider(provider: &mut Provider) {
    provider.id.zeroize();
    provider.name.zeroize();
    if let Some(category) = provider.category.as_mut() {
        category.zeroize();
    }
    zeroize_json_value(&mut provider.settings_config);
    for value in provider.extra.values_mut() {
        zeroize_json_value(value);
    }
    let Some(meta) = provider.meta.as_mut() else {
        return;
    };
    for endpoints in meta.custom_endpoints.iter_mut() {
        for value in endpoints.values_mut() {
            zeroize_json_value(value);
        }
    }
    for routes in meta.claude_desktop_model_routes.iter_mut() {
        for value in routes.values_mut() {
            zeroize_json_value(value);
        }
    }
    for value in [
        meta.usage_script.as_mut(),
        meta.test_config.as_mut(),
        meta.codex_chat_reasoning.as_mut(),
        meta.local_proxy_request_overrides.as_mut(),
    ]
    .into_iter()
    .flatten()
    {
        zeroize_json_value(value);
    }
    for value in [
        meta.claude_desktop_mode.as_mut(),
        meta.partner_promotion_key.as_mut(),
        meta.api_format.as_mut(),
        meta.provider_type.as_mut(),
        meta.github_account_id.as_mut(),
        meta.api_key_field.as_mut(),
        meta.custom_user_agent.as_mut(),
        meta.prompt_cache_key.as_mut(),
    ]
    .into_iter()
    .flatten()
    {
        value.zeroize();
    }
    if let Some(binding) = meta.auth_binding.as_mut() {
        for value in [
            binding.source.as_mut(),
            binding.auth_provider.as_mut(),
            binding.account_id.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            value.zeroize();
        }
    }
    for value in meta.extra.values_mut() {
        zeroize_json_value(value);
    }
}

fn zeroize_json_value(value: &mut Value) {
    match value {
        Value::String(value) => value.zeroize(),
        Value::Array(values) => {
            for value in values {
                zeroize_json_value(value);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                zeroize_json_value(value);
            }
        }
        _ => {}
    }
}

fn credential_value_at_slot(root: &Value, slot: &str) -> anyhow::Result<Value> {
    if let Some(toml_slot) = parse_toml_credential_slot(slot)? {
        let document = root
            .pointer("/settingsConfig/config")
            .and_then(Value::as_str)
            .context("TOML credential slot requires settingsConfig.config")?
            .parse::<toml::Value>()
            .context("parse Provider TOML credential source")?;
        let value = document
            .get("model_providers")
            .and_then(|root| root.get(&toml_slot.provider_id))
            .and_then(|root| root.get(&toml_slot.container))
            .and_then(|root| root.get(&toml_slot.name))
            .cloned()
            .with_context(|| format!("TOML credential slot does not exist: {slot}"))?;
        return serde_json::to_value(value).context("encode TOML credential slot");
    }
    root.pointer(slot)
        .cloned()
        .with_context(|| format!("credential slot does not exist: {slot}"))
}

fn set_credential_value_at_slot(
    root: &mut Value,
    slot: &str,
    replacement: Value,
) -> anyhow::Result<()> {
    if let Some(toml_slot) = parse_toml_credential_slot(slot)? {
        let config = root
            .pointer("/settingsConfig/config")
            .and_then(Value::as_str)
            .context("TOML credential slot requires settingsConfig.config")?;
        let mut document = config
            .parse::<toml::Value>()
            .context("parse redacted Provider TOML config")?;
        let replacement: toml::Value =
            serde_json::from_value(replacement).context("decode TOML credential slot")?;
        let container = document
            .get_mut("model_providers")
            .and_then(toml::Value::as_table_mut)
            .and_then(|providers| providers.get_mut(&toml_slot.provider_id))
            .and_then(toml::Value::as_table_mut)
            .and_then(|provider| provider.get_mut(&toml_slot.container))
            .and_then(toml::Value::as_table_mut)
            .with_context(|| format!("TOML credential container does not exist: {slot}"))?;
        let target = container
            .get_mut(&toml_slot.name)
            .with_context(|| format!("TOML credential slot does not exist: {slot}"))?;
        *target = replacement;
        return set_json_pointer(
            root,
            "/settingsConfig/config",
            Value::String(toml::to_string(&document).context("serialize Provider TOML config")?),
        );
    }
    if !provider_credential_slot_is_supported(slot) {
        anyhow::bail!("unsupported Provider credential slot {slot}");
    }
    set_json_pointer(root, slot, replacement)
}

pub fn merge_provider_credentials(
    existing: Option<&Provider>,
    incoming: &mut Provider,
    patches: &BTreeMap<String, CredentialPatch>,
) -> anyhow::Result<CredentialSummary> {
    let existing_value = existing
        .map(serde_json::to_value)
        .transpose()
        .context("serialize existing provider")?;
    let mut incoming_value =
        serde_json::to_value(&*incoming).context("serialize provider draft")?;

    restore_json_credentials(existing_value.as_ref(), &mut incoming_value)?;
    restore_toml_credentials(existing_value.as_ref(), &mut incoming_value)?;
    apply_credential_patches(existing_value.as_ref(), &mut incoming_value, patches)?;
    reject_unresolved_sentinels(&incoming_value)?;

    *incoming = serde_json::from_value(incoming_value).context("decode merged provider draft")?;
    let (_, summary) = redact_provider(incoming);
    Ok(summary)
}

pub fn reject_reserved_keep_sentinel(provider: &Provider) -> anyhow::Result<()> {
    let value = serde_json::to_value(provider).context("serialize provider")?;
    reject_unresolved_sentinels(&value)
}

fn redact_json_pointer(value: &mut Value, pointer: &str, slots: &mut BTreeSet<String>) {
    let Some(secret) = value.pointer_mut(pointer) else {
        return;
    };
    if value_is_configured(secret) {
        *secret = Value::String(SECRET_KEEP_SENTINEL.to_string());
        slots.insert(pointer.to_string());
    }
}

fn redact_dynamic_env(value: &mut Value, slots: &mut BTreeSet<String>) {
    let Some(env) = value
        .pointer_mut("/settingsConfig/env")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    for (key, item) in env {
        if looks_like_secret_key(key) && value_is_configured(item) {
            *item = Value::String(SECRET_KEEP_SENTINEL.to_string());
            slots.insert(format!("/settingsConfig/env/{}", escape_pointer(key)));
        }
    }
}

fn redact_unknown_secret_keys(value: &mut Value, pointer: &str, slots: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                let child = format!("{pointer}/{}", escape_pointer(key));
                if is_secret_container_key(key) {
                    if let Value::Object(values) = item {
                        for (name, value) in values {
                            if value_is_configured(value) {
                                let slot = format!("{child}/{}", escape_pointer(name));
                                *value = Value::String(SECRET_KEEP_SENTINEL.to_string());
                                slots.insert(slot);
                            }
                        }
                    } else if value_is_configured(item) {
                        *item = Value::String(SECRET_KEEP_SENTINEL.to_string());
                        slots.insert(child);
                    }
                    continue;
                }
                if is_known_non_secret_key(key) {
                    continue;
                }
                if looks_like_secret_key(key) && value_is_configured(item) {
                    *item = Value::String(SECRET_KEEP_SENTINEL.to_string());
                    slots.insert(child);
                } else {
                    redact_unknown_secret_keys(item, &child, slots);
                }
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                redact_unknown_secret_keys(item, &format!("{pointer}/{index}"), slots);
            }
        }
        _ => {}
    }
}

fn redact_toml_credentials(value: &mut Value, slots: &mut BTreeSet<String>) {
    let Some(config) = value
        .pointer("/settingsConfig/config")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return;
    };
    let Ok(mut document) = config.parse::<toml::Value>() else {
        if unparsed_toml_may_contain_credentials(&config) {
            if let Some(slot) = value.pointer_mut("/settingsConfig/config") {
                *slot = Value::String(SECRET_KEEP_SENTINEL.to_string());
                slots.insert("/settingsConfig/config".to_string());
            }
        }
        return;
    };
    if toml_contains_unsupported_credentials(&document, &[]) {
        if let Some(slot) = value.pointer_mut("/settingsConfig/config") {
            *slot = Value::String(SECRET_KEEP_SENTINEL.to_string());
            slots.insert("/settingsConfig/config".to_string());
        }
        return;
    }

    let mut changed = false;
    let Some(providers) = document
        .get_mut("model_providers")
        .and_then(toml::Value::as_table_mut)
    else {
        return;
    };
    for (provider_id, provider) in providers {
        let Some(provider) = provider.as_table_mut() else {
            continue;
        };
        for container_name in ["http_headers", "query_params"] {
            let Some(container) = provider
                .get_mut(container_name)
                .and_then(toml::Value::as_table_mut)
            else {
                continue;
            };
            for (name, item) in container {
                if toml_value_is_configured(item) {
                    *item = toml::Value::String(SECRET_KEEP_SENTINEL.to_string());
                    slots.insert(format!(
                        "/settingsConfig/config/model_providers/{}/{}/{}",
                        escape_pointer(provider_id),
                        container_name,
                        escape_pointer(name)
                    ));
                    changed = true;
                }
            }
        }
    }
    if !changed {
        return;
    }
    match toml::to_string(&document) {
        Ok(redacted) => {
            if let Some(slot) = value.pointer_mut("/settingsConfig/config") {
                *slot = Value::String(redacted);
            }
        }
        Err(_) => {
            if let Some(slot) = value.pointer_mut("/settingsConfig/config") {
                *slot = Value::String(SECRET_KEEP_SENTINEL.to_string());
                slots.insert("/settingsConfig/config".to_string());
            }
        }
    }
}

fn restore_json_credentials(existing: Option<&Value>, incoming: &mut Value) -> anyhow::Result<()> {
    let mut candidate_slots = BTreeSet::new();
    if let Some(existing) = existing {
        collect_json_secret_slots(existing, &mut candidate_slots);
    }
    collect_json_secret_slots(incoming, &mut candidate_slots);
    collect_sentinel_slots(incoming, "", &mut candidate_slots);

    for pointer in candidate_slots {
        if pointer.starts_with("/settingsConfig/config/model_providers/") {
            continue;
        }
        let incoming_secret = incoming.pointer(&pointer);
        let should_keep = incoming_secret.is_none_or(|value| {
            value
                .as_str()
                .is_some_and(|value| value.trim().is_empty() || value == SECRET_KEEP_SENTINEL)
        });
        if !should_keep {
            continue;
        }
        if let Some(existing_secret) = existing.and_then(|value| value.pointer(&pointer)) {
            set_json_pointer(incoming, &pointer, existing_secret.clone())?;
        } else if incoming_secret
            .and_then(Value::as_str)
            .is_some_and(|value| value == SECRET_KEEP_SENTINEL)
        {
            anyhow::bail!("credential keep requested for unknown slot {pointer}");
        }
    }
    Ok(())
}

fn collect_json_secret_slots(value: &Value, slots: &mut BTreeSet<String>) {
    for pointer in KNOWN_JSON_SECRET_POINTERS {
        if value.pointer(pointer).is_some() {
            slots.insert((*pointer).to_string());
        }
    }
    if let Some(env) = value
        .pointer("/settingsConfig/env")
        .and_then(Value::as_object)
    {
        for key in env.keys().filter(|key| looks_like_secret_key(key)) {
            slots.insert(format!("/settingsConfig/env/{}", escape_pointer(key)));
        }
    }
    collect_unknown_secret_slots(value, "", slots);
}

fn collect_unknown_secret_slots(value: &Value, pointer: &str, slots: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                let child = format!("{pointer}/{}", escape_pointer(key));
                if is_secret_container_key(key) {
                    if let Value::Object(values) = item {
                        for name in values.keys() {
                            slots.insert(format!("{child}/{}", escape_pointer(name)));
                        }
                    }
                    continue;
                }
                if !is_known_non_secret_key(key) && looks_like_secret_key(key) {
                    slots.insert(child);
                } else {
                    collect_unknown_secret_slots(item, &child, slots);
                }
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_unknown_secret_slots(item, &format!("{pointer}/{index}"), slots);
            }
        }
        _ => {}
    }
}

fn collect_sentinel_slots(value: &Value, pointer: &str, slots: &mut BTreeSet<String>) {
    match value {
        Value::String(item) if item == SECRET_KEEP_SENTINEL => {
            slots.insert(pointer.to_string());
        }
        Value::Object(map) => {
            for (key, item) in map {
                collect_sentinel_slots(item, &format!("{pointer}/{}", escape_pointer(key)), slots);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_sentinel_slots(item, &format!("{pointer}/{index}"), slots);
            }
        }
        _ => {}
    }
}

fn restore_toml_credentials(existing: Option<&Value>, incoming: &mut Value) -> anyhow::Result<()> {
    let Some(incoming_config) = incoming
        .pointer("/settingsConfig/config")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return Ok(());
    };
    let existing_config = existing
        .and_then(|value| value.pointer("/settingsConfig/config"))
        .and_then(Value::as_str);

    if incoming_config == SECRET_KEEP_SENTINEL {
        let existing_config =
            existing_config.context("credential keep requested without config")?;
        set_json_pointer(
            incoming,
            "/settingsConfig/config",
            Value::String(existing_config.to_string()),
        )?;
        return Ok(());
    }

    let Ok(mut incoming_toml) = incoming_config.parse::<toml::Value>() else {
        return Ok(());
    };
    let existing_toml = existing_config.and_then(|value| value.parse::<toml::Value>().ok());
    if let (Some(existing_config), Some(mut redacted_existing)) =
        (existing_config, existing_toml.clone())
    {
        redact_toml_values_for_comparison(&mut redacted_existing);
        if redacted_existing == incoming_toml {
            set_json_pointer(
                incoming,
                "/settingsConfig/config",
                Value::String(existing_config.to_string()),
            )?;
            return Ok(());
        }
    }
    let Some(providers) = incoming_toml
        .get_mut("model_providers")
        .and_then(toml::Value::as_table_mut)
    else {
        return Ok(());
    };
    let mut changed = false;
    for (provider_id, provider) in providers {
        let Some(provider) = provider.as_table_mut() else {
            continue;
        };
        for container_name in ["http_headers", "query_params"] {
            let Some(container) = provider
                .get_mut(container_name)
                .and_then(toml::Value::as_table_mut)
            else {
                continue;
            };
            for (name, item) in container {
                if item.as_str() != Some(SECRET_KEEP_SENTINEL) {
                    continue;
                }
                let replacement = existing_toml
                    .as_ref()
                    .and_then(|root| root.get("model_providers"))
                    .and_then(|root| root.get(provider_id.as_str()))
                    .and_then(|root| root.get(container_name))
                    .and_then(|root| root.get(name.as_str()))
                    .cloned()
                    .with_context(|| {
                        format!(
                            "credential keep requested for unknown TOML slot {provider_id}.{container_name}.{name}"
                        )
                    })?;
                *item = replacement;
                changed = true;
            }
        }
    }
    if changed {
        set_json_pointer(
            incoming,
            "/settingsConfig/config",
            Value::String(toml::to_string(&incoming_toml).context("serialize merged TOML config")?),
        )?;
    }
    Ok(())
}

fn redact_toml_values_for_comparison(document: &mut toml::Value) {
    let Some(providers) = document
        .get_mut("model_providers")
        .and_then(toml::Value::as_table_mut)
    else {
        return;
    };
    for (_, provider) in providers.iter_mut() {
        let Some(provider) = provider.as_table_mut() else {
            continue;
        };
        for container_name in ["http_headers", "query_params"] {
            let Some(container) = provider
                .get_mut(container_name)
                .and_then(toml::Value::as_table_mut)
            else {
                continue;
            };
            for (_, item) in container.iter_mut() {
                if toml_value_is_configured(item) {
                    *item = toml::Value::String(SECRET_KEEP_SENTINEL.to_string());
                }
            }
        }
    }
}

fn apply_credential_patches(
    existing: Option<&Value>,
    incoming: &mut Value,
    patches: &BTreeMap<String, CredentialPatch>,
) -> anyhow::Result<()> {
    for (pointer, patch) in patches {
        if !pointer.starts_with('/') {
            anyhow::bail!("credential patch slot must be a JSON pointer");
        }
        if let Some(slot) = parse_toml_credential_slot(pointer)? {
            apply_toml_credential_patch(existing, incoming, &slot, patch)?;
            continue;
        }
        if !is_allowed_json_credential_pointer(pointer) {
            anyhow::bail!("credential patch targets an unsupported slot {pointer}");
        }
        match patch {
            CredentialPatch::Keep => {
                let value = existing
                    .and_then(|value| value.pointer(pointer))
                    .cloned()
                    .with_context(|| {
                        format!("credential keep requested for unknown slot {pointer}")
                    })?;
                set_json_pointer(incoming, pointer, value)?;
            }
            CredentialPatch::Replace { value } => {
                if value.trim().is_empty() || value == SECRET_KEEP_SENTINEL {
                    anyhow::bail!("replacement credential must not be empty or reserved");
                }
                set_json_pointer(incoming, pointer, Value::String(value.clone()))?;
            }
            CredentialPatch::Clear => remove_json_pointer(incoming, pointer)?,
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TomlCredentialSlot {
    provider_id: String,
    container: String,
    name: String,
}

fn parse_toml_credential_slot(pointer: &str) -> anyhow::Result<Option<TomlCredentialSlot>> {
    const PREFIX: &str = "/settingsConfig/config/model_providers/";
    let Some(path) = pointer.strip_prefix(PREFIX) else {
        return Ok(None);
    };
    let segments = path.split('/').map(unescape_pointer).collect::<Vec<_>>();
    if segments.len() != 3
        || segments.iter().any(|segment| segment.trim().is_empty())
        || !matches!(segments[1].as_str(), "http_headers" | "query_params")
    {
        anyhow::bail!("invalid TOML credential slot {pointer}");
    }
    Ok(Some(TomlCredentialSlot {
        provider_id: segments[0].clone(),
        container: segments[1].clone(),
        name: segments[2].clone(),
    }))
}

fn apply_toml_credential_patch(
    existing: Option<&Value>,
    incoming: &mut Value,
    slot: &TomlCredentialSlot,
    patch: &CredentialPatch,
) -> anyhow::Result<()> {
    let incoming_config = incoming
        .pointer("/settingsConfig/config")
        .and_then(Value::as_str)
        .context("TOML credential patch requires settingsConfig.config")?;
    let mut document = incoming_config
        .parse::<toml::Value>()
        .context("TOML credential patch requires valid settingsConfig.config")?;

    let replacement = match patch {
        CredentialPatch::Keep => Some(
            existing
                .and_then(|value| value.pointer("/settingsConfig/config"))
                .and_then(Value::as_str)
                .and_then(|config| config.parse::<toml::Value>().ok())
                .as_ref()
                .and_then(|root| root.get("model_providers"))
                .and_then(|root| root.get(&slot.provider_id))
                .and_then(|root| root.get(&slot.container))
                .and_then(|root| root.get(&slot.name))
                .cloned()
                .with_context(|| {
                    format!(
                        "credential keep requested for unknown TOML slot {}.{}.{}",
                        slot.provider_id, slot.container, slot.name
                    )
                })?,
        ),
        CredentialPatch::Replace { value } => {
            if value.trim().is_empty() || value == SECRET_KEEP_SENTINEL {
                anyhow::bail!("replacement credential must not be empty or reserved");
            }
            Some(toml::Value::String(value.clone()))
        }
        CredentialPatch::Clear => None,
    };

    let provider = document
        .get_mut("model_providers")
        .and_then(toml::Value::as_table_mut)
        .and_then(|providers| providers.get_mut(&slot.provider_id))
        .and_then(toml::Value::as_table_mut)
        .with_context(|| {
            format!(
                "TOML credential provider does not exist: {}",
                slot.provider_id
            )
        })?;
    let container = provider
        .entry(slot.container.clone())
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .with_context(|| {
            format!(
                "TOML credential container is not a table: {}.{}",
                slot.provider_id, slot.container
            )
        })?;
    if let Some(replacement) = replacement {
        container.insert(slot.name.clone(), replacement);
    } else {
        container.remove(&slot.name);
    }

    set_json_pointer(
        incoming,
        "/settingsConfig/config",
        Value::String(toml::to_string(&document).context("serialize patched TOML config")?),
    )
}

fn is_allowed_json_credential_pointer(pointer: &str) -> bool {
    if KNOWN_JSON_SECRET_POINTERS.contains(&pointer) {
        return true;
    }
    let Some((parent, leaf)) = pointer.rsplit_once('/') else {
        return false;
    };
    let leaf = unescape_pointer(leaf);
    if parent == "/settingsConfig/env" {
        return looks_like_secret_key(&leaf);
    }
    matches!(
        parent.rsplit('/').next(),
        Some("httpHeaders" | "http_headers" | "queryParams" | "query_params" | "extraHeaders")
    ) || looks_like_secret_key(&leaf)
}

fn set_json_pointer(root: &mut Value, pointer: &str, replacement: Value) -> anyhow::Result<()> {
    if let Some(slot) = root.pointer_mut(pointer) {
        *slot = replacement;
        return Ok(());
    }
    let (parent, leaf) = pointer
        .rsplit_once('/')
        .context("credential slot has no parent")?;
    let leaf = unescape_pointer(leaf);
    let parent = root
        .pointer_mut(parent)
        .and_then(Value::as_object_mut)
        .with_context(|| format!("credential slot parent does not exist: {parent}"))?;
    parent.insert(leaf, replacement);
    Ok(())
}

fn remove_json_pointer(root: &mut Value, pointer: &str) -> anyhow::Result<()> {
    let (parent, leaf) = pointer
        .rsplit_once('/')
        .context("credential slot has no parent")?;
    let leaf = unescape_pointer(leaf);
    let parent = root
        .pointer_mut(parent)
        .and_then(Value::as_object_mut)
        .with_context(|| format!("credential slot parent does not exist: {parent}"))?;
    parent.remove(&leaf);
    Ok(())
}

fn reject_unresolved_sentinels(value: &Value) -> anyhow::Result<()> {
    let mut slots = BTreeSet::new();
    collect_sentinel_slots(value, "", &mut slots);
    if let Some(slot) = slots.into_iter().next() {
        anyhow::bail!("unresolved credential keep sentinel at {slot}");
    }
    Ok(())
}

fn value_is_configured(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(value) => !value.trim().is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
        _ => true,
    }
}

fn toml_value_is_configured(value: &toml::Value) -> bool {
    value.as_str().is_none_or(|value| !value.trim().is_empty())
}

fn looks_like_secret_key(key: &str) -> bool {
    let mut normalized = String::with_capacity(key.len() + 4);
    for (index, character) in key.chars().enumerate() {
        if character.is_ascii_uppercase() && index > 0 {
            normalized.push('_');
        }
        normalized.push(if character == '-' || character == ' ' {
            '_'
        } else {
            character.to_ascii_lowercase()
        });
    }
    let compact = normalized.replace('_', "");
    matches!(
        compact.as_str(),
        "apikey"
            | "accesskey"
            | "secret"
            | "clientsecret"
            | "password"
            | "authorization"
            | "authtoken"
            | "accesstoken"
            | "refreshtoken"
            | "sessiontoken"
            | "bearertoken"
            | "token"
            | "privatekey"
            | "signingkey"
            | "credential"
            | "credentials"
            | "cookie"
            | "sessioncookie"
    ) || [
        "apikey",
        "accesskeyid",
        "secretaccesskey",
        "sessiontoken",
        "authtoken",
        "accesstoken",
        "refreshtoken",
        "password",
        "privatekey",
        "signingkey",
        "clientsecret",
    ]
    .iter()
    .any(|suffix| compact.ends_with(suffix))
}

fn is_known_non_secret_key(key: &str) -> bool {
    matches!(
        key,
        "apiKeyField"
            | "api_key_field"
            | "tokenLimit"
            | "token_limit"
            | "maxTokens"
            | "max_tokens"
            | "promptCacheKey"
            | "prompt_cache_key"
    )
}

fn is_secret_container_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().replace(['-', '_'], "").as_str(),
        "auth"
            | "credentials"
            | "secrets"
            | "httpheaders"
            | "queryparams"
            | "extraheaders"
            | "cookies"
    )
}

fn toml_contains_unsupported_credentials(value: &toml::Value, path: &[&str]) -> bool {
    let Some(table) = value.as_table() else {
        return false;
    };
    for (key, item) in table {
        let supported_container = path.len() == 2
            && path[0] == "model_providers"
            && matches!(key.as_str(), "http_headers" | "query_params");
        if supported_container {
            continue;
        }
        if looks_like_secret_key(key) || is_secret_container_key(key) {
            return true;
        }
        let mut child = path.to_vec();
        child.push(key);
        if toml_contains_unsupported_credentials(item, &child) {
            return true;
        }
    }
    false
}

fn unparsed_toml_may_contain_credentials(config: &str) -> bool {
    config.lines().any(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return false;
        }
        let key = trimmed
            .split_once('=')
            .map(|(key, _)| key.trim().trim_matches(['"', '\'']))
            .unwrap_or(trimmed);
        looks_like_secret_key(key)
            || is_secret_container_key(key)
            || trimmed.to_ascii_lowercase().contains("bearer ")
    })
}

fn escape_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn unescape_pointer(value: &str) -> String {
    value.replace("~1", "/").replace("~0", "~")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn provider() -> Provider {
        Provider {
            id: "provider-1".to_string(),
            name: "Provider".to_string(),
            settings_config: json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://example.com",
                    "ANTHROPIC_AUTH_TOKEN": "secret-anthropic",
                    "CUSTOM_PASSWORD": "secret-password",
                    "MAX_TOKENS": "4096"
                },
                "auth": {"OPENAI_API_KEY": "secret-openai"},
                "config": "model = \"gpt-test\"\n[model_providers.demo.http_headers]\nAuthorization = \"Bearer secret\"\n[model_providers.demo.query_params]\nkey = \"secret-query\"\n"
            }),
            category: None,
            meta: None,
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn redacts_json_and_toml_credentials_without_hiding_runtime_fields() {
        let (redacted, summary) = redact_provider(&provider());
        let serialized = serde_json::to_string(&redacted).unwrap();

        assert!(summary.configured);
        assert!(!serialized.contains("secret-anthropic"));
        assert!(!serialized.contains("secret-password"));
        assert!(!serialized.contains("secret-openai"));
        assert!(!serialized.contains("secret-query"));
        assert!(serialized.contains("https://example.com"));
        assert!(serialized.contains("4096"));
        assert!(summary
            .slots
            .iter()
            .any(|slot| slot.contains("http_headers")));
    }

    #[test]
    fn reveals_only_configured_credential_slots() {
        let provider = provider();

        assert_eq!(
            reveal_provider_credential(&provider, "/settingsConfig/env/ANTHROPIC_AUTH_TOKEN")
                .unwrap(),
            "secret-anthropic"
        );
        assert_eq!(
            reveal_provider_credential(
                &provider,
                "/settingsConfig/config/model_providers/demo/query_params/key"
            )
            .unwrap(),
            "secret-query"
        );

        let error = reveal_provider_credential(&provider, "/name").unwrap_err();
        assert!(error.to_string().contains("not configured"));
    }

    #[test]
    fn redacts_camel_case_and_unknown_credential_containers() {
        let mut provider = provider();
        provider
            .extra
            .insert("privateKey".to_string(), json!("private-value"));
        provider.settings_config["credentials"] = json!({
            "opaque": "credential-value",
            "refreshToken": "refresh-value"
        });
        provider.meta = Some(super::super::model::ProviderMeta {
            extra: BTreeMap::from([("sessionCookie".to_string(), json!("cookie-value"))]),
            ..Default::default()
        });

        let (redacted, summary) = redact_provider(&provider);
        let serialized = serde_json::to_string(&redacted).unwrap();

        for secret in [
            "private-value",
            "credential-value",
            "refresh-value",
            "cookie-value",
        ] {
            assert!(!serialized.contains(secret));
        }
        assert!(summary
            .slots
            .iter()
            .any(|slot| slot.ends_with("/privateKey")));
        assert!(summary
            .slots
            .iter()
            .any(|slot| slot.contains("/credentials/opaque")));
    }

    #[test]
    fn unsupported_toml_credentials_fail_closed_to_whole_config_keep() {
        let mut existing = provider();
        existing.settings_config["config"] =
            json!("model = \"gpt-test\"\n[custom_auth]\nprivate_key = \"toml-private-value\"\n");

        let (mut redacted, summary) = redact_provider(&existing);

        assert_eq!(
            redacted.settings_config["config"],
            json!(SECRET_KEEP_SENTINEL)
        );
        assert!(summary
            .slots
            .iter()
            .any(|slot| slot == "/settingsConfig/config"));
        assert!(!serde_json::to_string(&redacted)
            .unwrap()
            .contains("toml-private-value"));

        redacted.name = "Renamed".to_string();
        merge_provider_credentials(Some(&existing), &mut redacted, &BTreeMap::new()).unwrap();
        assert_eq!(redacted.name, "Renamed");
        assert_eq!(
            redacted.settings_config["config"],
            existing.settings_config["config"]
        );
    }

    #[test]
    fn keep_sentinel_restores_existing_credentials() {
        let existing = provider();
        let (mut draft, _) = redact_provider(&existing);
        draft.name = "Renamed".to_string();

        merge_provider_credentials(Some(&existing), &mut draft, &BTreeMap::new()).unwrap();

        assert_eq!(draft.name, "Renamed");
        assert_eq!(draft.settings_config, existing.settings_config);
    }

    #[test]
    fn explicit_clear_and_replace_are_distinct_from_keep() {
        let existing = provider();
        let (mut draft, _) = redact_provider(&existing);
        let patches = BTreeMap::from([
            (
                "/settingsConfig/env/ANTHROPIC_AUTH_TOKEN".to_string(),
                CredentialPatch::Clear,
            ),
            (
                "/settingsConfig/auth/OPENAI_API_KEY".to_string(),
                CredentialPatch::Replace {
                    value: "replacement".to_string(),
                },
            ),
        ]);

        merge_provider_credentials(Some(&existing), &mut draft, &patches).unwrap();

        assert!(draft.settings_config["env"]
            .get("ANTHROPIC_AUTH_TOKEN")
            .is_none());
        assert_eq!(
            draft.settings_config["auth"]["OPENAI_API_KEY"],
            json!("replacement")
        );
    }

    #[test]
    fn toml_slots_support_keep_replace_and_clear() {
        let existing = provider();
        let (mut draft, summary) = redact_provider(&existing);
        let header_slot = summary
            .slots
            .iter()
            .find(|slot| slot.contains("/http_headers/Authorization"))
            .unwrap()
            .clone();
        let query_slot = summary
            .slots
            .iter()
            .find(|slot| slot.contains("/query_params/key"))
            .unwrap()
            .clone();
        let patches = BTreeMap::from([
            (
                header_slot,
                CredentialPatch::Replace {
                    value: "Bearer replacement".to_string(),
                },
            ),
            (query_slot, CredentialPatch::Clear),
        ]);

        merge_provider_credentials(Some(&existing), &mut draft, &patches).unwrap();

        let config = draft.settings_config["config"].as_str().unwrap();
        let config = config.parse::<toml::Value>().unwrap();
        assert_eq!(
            config["model_providers"]["demo"]["http_headers"]["Authorization"].as_str(),
            Some("Bearer replacement")
        );
        assert!(config["model_providers"]["demo"]["query_params"]
            .get("key")
            .is_none());
    }

    #[test]
    fn credential_patch_rejects_non_secret_fields() {
        let existing = provider();
        let (mut draft, _) = redact_provider(&existing);
        let patches = BTreeMap::from([(
            "/name".to_string(),
            CredentialPatch::Replace {
                value: "not-a-credential".to_string(),
            },
        )]);

        let error = merge_provider_credentials(Some(&existing), &mut draft, &patches).unwrap_err();

        assert!(error.to_string().contains("unsupported slot"));
    }

    #[test]
    fn create_rejects_keep_sentinel_without_existing_secret() {
        let (mut draft, _) = redact_provider(&provider());
        let error = merge_provider_credentials(None, &mut draft, &BTreeMap::new()).unwrap_err();
        assert!(error.to_string().contains("unknown slot"));
    }
}
