//! Display-only projection. Protocol/App/credential bindings are not provider brands.
use std::{collections::BTreeMap, sync::OnceLock};

use serde::{Deserialize, Serialize};

use super::{bundle::bundle_family_id, registry, store::StoredProvider};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderIdentity {
    pub schema_version: u32,
    pub family_id: String,
    pub category: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_color: Option<String>,
    pub catalog_version: String,
    pub revision: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Catalog {
    catalog_version: String,
    families: BTreeMap<String, Family>,
}

#[derive(Deserialize)]
struct Family {
    category: String,
    icon: String,
}

fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../../assets/generated/provider-presentation.json"
        ))
        .expect("validated provider presentation catalog")
    })
}

pub fn provider_identity(provider: &StoredProvider) -> Option<ProviderIdentity> {
    let declared = bundle_family_id(&provider.provider).and_then(registry::family_by_id);
    let registered = provider
        .resource
        .profile_id
        .as_ref()
        .and_then(|id| registry::family_for_profile(id.as_str()));
    if declared
        .zip(registered)
        .is_some_and(|(a, b)| a.family_id != b.family_id)
    {
        return None;
    }
    let family = registered.or(declared)?;
    let presentation = catalog().families.get(&family.family_id)?;
    let extra_string = |key: &str| {
        provider
            .provider
            .extra
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    Some(ProviderIdentity {
        schema_version: 1,
        family_id: family.family_id.clone(),
        category: presentation.category.clone(),
        display_name: provider.provider.name.clone(),
        icon: Some(extra_string("icon").unwrap_or_else(|| presentation.icon.clone())),
        icon_color: extra_string("iconColor"),
        catalog_version: catalog().catalog_version.clone(),
        revision: provider.resource.revision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentation_covers_exactly_the_registry_not_app_surfaces() {
        let families = &registry::provider_registry().families;
        assert_eq!(catalog().families.len(), families.len());
        for family in families {
            assert!(catalog().families.contains_key(&family.family_id));
        }
        assert_eq!(catalog().families["family.minimax_cn"].category, "minimax");
        assert_eq!(catalog().families["family.custom_http"].category, "custom");
    }

    #[test]
    fn generated_wire_fixtures_match_every_registry_surface() {
        let fixtures: Vec<serde_json::Value> = serde_json::from_str(include_str!(
            "../../../assets/generated/provider-identity-fixtures.json"
        ))
        .unwrap();
        for fixture in fixtures {
            let family = registry::family_by_id(fixture["name"].as_str().unwrap()).unwrap();
            for surface in &family.surfaces {
                let expected =
                    &fixture["appProviders"][surface.app.as_str()][0]["providerIdentity"];
                let stored = StoredProvider {
                    app: surface.app,
                    provider: serde_json::from_value(serde_json::json!({
                        "id": "fixture-provider", "name": family.label, "iconColor": "#00A67E"
                    }))
                    .unwrap(),
                    provider_type: super::super::model::ProviderType::Codex,
                    provider_type_id: "codex".into(),
                    resource: super::super::store::ProviderResourceMetadata {
                        profile_id: Some(surface.profile_id.clone()),
                        revision: 7,
                        ..Default::default()
                    },
                };
                assert_eq!(
                    serde_json::to_value(provider_identity(&stored).unwrap()).unwrap(),
                    *expected,
                    "{} {}",
                    family.family_id,
                    surface.app.as_str()
                );
            }
        }
    }
}
