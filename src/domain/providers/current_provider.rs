use serde_json::Value;

use crate::domain::providers::model::AppKind;
use crate::domain::providers::store::ProviderStore;

pub fn current_provider_settings_key(app: AppKind) -> &'static str {
    match app {
        AppKind::Claude => "currentProviderClaude",
        AppKind::Codex => "currentProviderCodex",
        AppKind::Gemini => "currentProviderGemini",
    }
}

pub fn read_current_provider_id(ui_settings: &Value, app: AppKind) -> Option<String> {
    ui_settings
        .get(current_provider_settings_key(app))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn has_current_provider_setting(ui_settings: &Value, app: AppKind) -> bool {
    ui_settings
        .get(current_provider_settings_key(app))
        .is_some()
}

/// Resolve the effective current provider id for routing UI, matching
/// `get_current_provider` invoke semantics.
pub fn resolve_current_provider_id(
    store: &ProviderStore,
    ui_settings: &Value,
    app: AppKind,
) -> Option<String> {
    let provider_exists = |id: &str| {
        store
            .providers
            .iter()
            .any(|provider| provider.app == app && provider.provider.id == id)
    };
    if has_current_provider_setting(ui_settings, app) {
        read_current_provider_id(ui_settings, app).filter(|id| provider_exists(id))
    } else {
        read_current_provider_id(ui_settings, app)
            .filter(|id| provider_exists(id))
            .or_else(|| {
                store
                    .list(Some(app))
                    .into_iter()
                    .next()
                    .map(|provider| provider.provider.id)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::providers::model::Provider;
    use crate::domain::providers::store::ProviderStore;
    use serde_json::json;

    #[test]
    fn has_current_provider_setting_detects_explicit_clear() {
        let ui_settings = json!({ "currentProviderClaude": "" });
        assert!(has_current_provider_setting(&ui_settings, AppKind::Claude));
        assert_eq!(
            read_current_provider_id(&ui_settings, AppKind::Claude),
            None
        );
        let unset = json!({});
        assert!(!has_current_provider_setting(&unset, AppKind::Claude));
    }

    #[test]
    fn resolve_current_provider_id_falls_back_to_first_sorted_provider() {
        let ui_settings = json!({});
        let mut store = ProviderStore::default();
        store.upsert(
            AppKind::Claude,
            Provider {
                id: "p1".to_string(),
                name: "first".to_string(),
                settings_config: json!({}),
                category: None,
                meta: None,
                extra: Default::default(),
            },
        );
        assert_eq!(
            resolve_current_provider_id(&store, &ui_settings, AppKind::Claude).as_deref(),
            Some("p1")
        );
    }

    #[test]
    fn resolve_current_provider_id_respects_explicit_clear_without_fallback() {
        let ui_settings = json!({ "currentProviderClaude": "" });
        let mut store = ProviderStore::default();
        store.upsert(
            AppKind::Claude,
            Provider {
                id: "p1".to_string(),
                name: "first".to_string(),
                settings_config: json!({}),
                category: None,
                meta: None,
                extra: Default::default(),
            },
        );
        assert_eq!(
            resolve_current_provider_id(&store, &ui_settings, AppKind::Claude),
            None
        );
    }
}
