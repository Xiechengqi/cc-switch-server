use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::credentials::{CredentialPatch, ProviderView};
use super::model::{AppKind, Provider, ProviderMeta};
use super::model_routing::{
    normalize_and_validate_provider_model_routing, policy_from_settings, ModelRoutingPolicy,
};
use super::registry::{
    family_by_id, family_for_profile, profile_by_id, CredentialPolicy, CustomBindingInput,
    ProfileId, ProviderFamilySpec,
};
use super::store::StoredProvider;

const BUNDLE_ID_FIELD: &str = "bundleId";
const FAMILY_ID_FIELD: &str = "familyId";
const ROUTE_KEY_FIELD: &str = "routeKey";
const SURFACE_ENABLED_FIELD: &str = "surfaceEnabled";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderBundleView {
    pub id: String,
    pub family_id: String,
    pub route_key: String,
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
    pub route_key: String,
    pub name: String,
    #[serde(default)]
    pub website_url: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub icon_color: Option<String>,
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
pub struct ProviderBundleSurfaceWriteDraft {
    pub app: AppKind,
    pub enabled: bool,
    pub profile_id: ProfileId,
    #[serde(default)]
    pub settings_config: Value,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub meta: Option<ProviderMeta>,
    #[serde(default)]
    pub custom_binding: Option<CustomBindingInput>,
    #[serde(default)]
    pub credential_patches: BTreeMap<String, CredentialPatch>,
}

impl ProviderBundleWriteDraft {
    pub fn validate(&self) -> anyhow::Result<&'static ProviderFamilySpec> {
        validate_bundle_id(&self.id)?;
        validate_route_key(&self.route_key)?;
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
        let mut bundle_model_policy: Option<ModelRoutingPolicy> = None;
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
            let mut provider = self.provider_for_surface(surface);
            normalize_and_validate_provider_model_routing(
                surface.app,
                &mut provider,
                Some(profile),
            )?;
            let surface_model_policy = policy_from_settings(&provider.settings_config)
                .context("Provider Bundle Surface has no resolved model policy")?;
            if bundle_model_policy
                .as_ref()
                .is_some_and(|policy| policy != &surface_model_policy)
            {
                bail!("Provider Bundle Surfaces must share one model policy and upstream model");
            }
            bundle_model_policy.get_or_insert(surface_model_policy);
            enabled += usize::from(surface.enabled);
        }
        if enabled == 0 {
            bail!("Provider Bundle must enable at least one Surface");
        }
        Ok(family)
    }

    pub fn provider_for_surface(&self, surface: &ProviderBundleSurfaceWriteDraft) -> Provider {
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
            ROUTE_KEY_FIELD.to_string(),
            Value::String(self.route_key.clone()),
        );
        extra.insert(
            SURFACE_ENABLED_FIELD.to_string(),
            Value::Bool(surface.enabled),
        );
        Provider {
            id: self.id.clone(),
            name: self.name.clone(),
            settings_config: surface.settings_config.clone(),
            category: surface.category.clone(),
            meta: surface.meta.clone(),
            extra,
        }
    }
}

impl ProviderBundleView {
    pub fn from_surface_views(surface_views: Vec<ProviderView>) -> anyhow::Result<Self> {
        let first = surface_views
            .first()
            .context("Provider Bundle has no Surfaces")?;
        let id = bundle_id(&first.provider).to_string();
        let family_id = family_id_for_view(first)?;
        let bundle_route_key = route_key(&first.provider).to_string();
        let name = first.provider.name.clone();
        let mut revision = 0u64;
        let mut surfaces = BTreeMap::new();
        let mut enabled_apps = Vec::new();
        let mut enabled_credential_states = Vec::new();
        let mut credential_slots = BTreeSet::new();
        for view in surface_views {
            if bundle_id(&view.provider) != id
                || family_id_for_view(&view)? != family_id
                || route_key(&view.provider) != bundle_route_key
                || view.provider.name != name
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
        Ok(Self {
            id,
            family_id,
            route_key: bundle_route_key,
            revision,
            name,
            website_url: extra_string(provider, "websiteUrl"),
            notes: extra_string(provider, "notes"),
            icon: extra_string(provider, "icon"),
            icon_color: extra_string(provider, "iconColor"),
            supported_apps,
            enabled_apps,
            credential_configured,
            credential_slots: credential_slots.into_iter().collect(),
            surfaces,
        })
    }
}

pub fn bundle_id(provider: &Provider) -> &str {
    extra_string_ref(provider, BUNDLE_ID_FIELD).unwrap_or(&provider.id)
}

pub fn has_bundle_managed_metadata(provider: &Provider) -> bool {
    [
        BUNDLE_ID_FIELD,
        FAMILY_ID_FIELD,
        ROUTE_KEY_FIELD,
        SURFACE_ENABLED_FIELD,
    ]
    .iter()
    .any(|field| provider.extra.contains_key(*field))
}

pub fn is_explicit_bundle_surface(provider: &Provider) -> bool {
    extra_string_ref(provider, BUNDLE_ID_FIELD).is_some()
        && extra_string_ref(provider, FAMILY_ID_FIELD).is_some()
        && extra_string_ref(provider, ROUTE_KEY_FIELD).is_some()
        && provider.extra.contains_key(SURFACE_ENABLED_FIELD)
}

pub fn route_key(provider: &Provider) -> &str {
    extra_string_ref(provider, ROUTE_KEY_FIELD).unwrap_or(&provider.id)
}

pub fn surface_enabled(provider: &Provider) -> bool {
    provider
        .extra
        .get(SURFACE_ENABLED_FIELD)
        .and_then(Value::as_bool)
        .unwrap_or(true)
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
    let Some(family_id) = extra_string_ref(&stored.provider, FAMILY_ID_FIELD) else {
        return Ok(None);
    };
    let family =
        family_by_id(family_id).with_context(|| format!("unknown Provider family {family_id}"))?;
    if family.surfaces.len() < 2 {
        return Ok(None);
    }
    let credential_profile =
        profile_by_id(family.credential_profile_id.as_str()).with_context(|| {
            format!(
                "unknown credential profile {}",
                family.credential_profile_id
            )
        })?;
    if matches!(
        credential_profile.credential_policy,
        CredentialPolicy::ManagedAccount { .. }
            | CredentialPolicy::Custom
            | CredentialPolicy::Legacy
    ) {
        return Ok(None);
    }
    Ok(Some(super::registry::ProviderKey::new(
        credential_source_app(family)?,
        bundle_id(&stored.provider),
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

pub fn validate_route_key(value: &str) -> anyhow::Result<()> {
    let valid = (3..=64).contains(&value.len())
        && value == value.trim()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
        && value.bytes().any(|byte| byte.is_ascii_lowercase());
    if !valid {
        bail!("routeKey must be 3-64 lowercase letters, digits, hyphens, or underscores");
    }
    Ok(())
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

    fn grok_surface(
        app: AppKind,
        profile_id: &str,
        upstream_model: &str,
    ) -> ProviderBundleSurfaceWriteDraft {
        ProviderBundleSurfaceWriteDraft {
            app,
            enabled: true,
            profile_id: ProfileId::parse(profile_id).unwrap(),
            settings_config: json!({
                "modelMapping": {
                    "mode": "single",
                    "upstreamModel": upstream_model,
                }
            }),
            category: None,
            meta: None,
            custom_binding: None,
            credential_patches: BTreeMap::new(),
        }
    }

    fn grok_bundle(upstream_models: [&str; 3]) -> ProviderBundleWriteDraft {
        ProviderBundleWriteDraft {
            id: "grok-bundle".to_string(),
            family_id: "family.grok_oauth".to_string(),
            route_key: "grok-bundle".to_string(),
            name: "Grok Bundle".to_string(),
            website_url: None,
            notes: None,
            icon: None,
            icon_color: None,
            surfaces: vec![
                grok_surface(AppKind::Claude, "claude.grok_oauth", upstream_models[0]),
                grok_surface(AppKind::Codex, "codex.grok_oauth", upstream_models[1]),
                grok_surface(AppKind::Gemini, "gemini.grok_oauth", upstream_models[2]),
            ],
            credential_patches: BTreeMap::new(),
            expected_revision: None,
            client_request_id: None,
        }
    }

    #[test]
    fn bundle_surfaces_require_one_model_policy_and_upstream_model() {
        assert!(grok_bundle(["grok-4.5"; 3]).validate().is_ok());

        let error = grok_bundle(["grok-4.5", "grok-4.5", "grok-other"])
            .validate()
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("must share one model policy and upstream model"));
    }
}
