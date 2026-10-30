//! Automatic monitoring policy. Selection never depends on a previous probe's
//! success: a failed selected interface must not be hidden by trying another.
use super::bundle::{bundle_id, surface_enabled};
use super::model::AppKind;
use super::registry::{
    profile_by_id, provider_registry, resolve_custom_binding, CustomBindingInput, DriverBinding,
    OperationSupport, ProfileSpec,
};
use super::runtime::compile_runtime_plan_with_defaults;
use super::store::{ProviderStore, StoredProvider};
use crate::domain::accounts::store::AccountStore;
use crate::domain::health::{provider_probe_support, ProviderProbeSupport};

pub const AUTOMATIC_PROBE_APP_ORDER: [AppKind; 3] =
    [AppKind::Codex, AppKind::Claude, AppKind::Gemini];

pub fn automatic_probe_app(mut eligible: impl FnMut(AppKind) -> bool) -> Option<AppKind> {
    AUTOMATIC_PROBE_APP_ORDER
        .into_iter()
        .find(|app| eligible(*app))
}

pub fn profile_supports_probe(
    profile: &ProfileSpec,
    custom_binding: Option<&CustomBindingInput>,
) -> bool {
    let driver_id = match &profile.driver_binding {
        DriverBinding::Fixed { driver_id } => driver_id.clone(),
        DriverBinding::Custom { .. } => {
            let Some(binding) = custom_binding else {
                return false;
            };
            let Ok(binding) = resolve_custom_binding(profile, binding) else {
                return false;
            };
            binding.driver_id
        }
    };
    provider_registry().drivers.iter().any(|driver| {
        driver.driver_id == driver_id && driver.operations.test == OperationSupport::Supported
    })
}

pub fn provider_has_probe(store: &ProviderStore, provider: &StoredProvider) -> bool {
    if !surface_enabled(&provider.provider) {
        return false;
    }
    // Real stores have a compiled index. The fallback also supports offline
    // snapshots/legacy readers without inventing a probe for an unknown Driver.
    let plan = store
        .runtime_plan(provider.app, &provider.provider.id)
        .or_else(|| {
            compile_runtime_plan_with_defaults(
                provider,
                &AccountStore::default(),
                store.runtime_defaults(),
            )
            .ok()
            .map(std::sync::Arc::new)
        });
    plan.is_some_and(|plan| {
        plan.provider_revision == provider.resource.revision
            && provider_probe_support(&plan) == ProviderProbeSupport::Supported
            && plan
                .test_model
                .as_deref()
                .is_some_and(|model| !model.trim().is_empty())
    })
}

pub fn bundle_probe_app(store: &ProviderStore, id: &str) -> Option<AppKind> {
    automatic_probe_app(|app| {
        store.providers.iter().any(|provider| {
            provider.app == app
                && bundle_id(&provider.provider) == Some(id)
                && provider_has_probe(store, provider)
        })
    })
}

pub fn profile_id_supports_probe(id: &str, binding: Option<&CustomBindingInput>) -> bool {
    profile_by_id(id).is_some_and(|profile| profile_supports_probe(profile, binding))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_priority_is_fixed_and_skips_only_ineligible_apps() {
        assert_eq!(automatic_probe_app(|_| true), Some(AppKind::Codex));
        assert_eq!(
            automatic_probe_app(|app| app != AppKind::Codex),
            Some(AppKind::Claude)
        );
        assert_eq!(
            automatic_probe_app(|app| app == AppKind::Gemini),
            Some(AppKind::Gemini)
        );
        assert_eq!(automatic_probe_app(|_| false), None);
    }
}
