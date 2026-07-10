use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;

use crate::domain::providers::model::Provider;
use crate::domain::providers::store::ProviderStore;

const MIGRATION_MARKER: &str = ".migrated-universal-layer";
const UNIVERSAL_PROVIDERS_FILE: &str = "universal-providers.json";

pub fn migrate_remove_universal_layer(config_dir: &Path) -> anyhow::Result<bool> {
    let marker = config_dir.join(MIGRATION_MARKER);
    if marker.exists() {
        return Ok(false);
    }

    let universal_path = config_dir.join(UNIVERSAL_PROVIDERS_FILE);
    let mut providers = ProviderStore::load_or_default(config_dir)?;
    let orphaned = orphan_universal_derivatives(&mut providers);
    let had_universal_file = universal_path.exists();

    if orphaned > 0 {
        providers
            .save(config_dir)
            .context("save providers after universal orphan migration")?;
    }

    if had_universal_file {
        archive_universal_providers_file(config_dir, &universal_path)?;
    }

    if orphaned > 0 || had_universal_file {
        fs::write(&marker, b"1").with_context(|| format!("write {}", marker.display()))?;
        tracing::info!(
            orphaned_providers = orphaned,
            archived_universal_file = had_universal_file,
            "removed universal provider layer"
        );
        return Ok(true);
    }

    fs::write(&marker, b"1").with_context(|| format!("write {}", marker.display()))?;
    Ok(false)
}

pub fn orphan_universal_derivatives(store: &mut ProviderStore) -> usize {
    let mut count = 0usize;
    for item in &mut store.providers {
        if orphan_provider(&mut item.provider) {
            count += 1;
        }
    }
    count
}

fn orphan_provider(provider: &mut Provider) -> bool {
    let had_link = provider.extra.remove("universalProviderId").is_some()
        || provider
            .meta
            .as_mut()
            .and_then(|meta| meta.extra.remove("universalProviderId"))
            .is_some();
    let is_universal_id = provider.id.starts_with("universal:");
    if !had_link && !is_universal_id && provider.category.as_deref() != Some("universal") {
        return false;
    }
    if provider.category.as_deref() == Some("universal") {
        provider.category = Some("custom".to_string());
    }
    true
}

fn archive_universal_providers_file(
    config_dir: &Path,
    universal_path: &Path,
) -> anyhow::Result<()> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let archive = config_dir.join(format!("{UNIVERSAL_PROVIDERS_FILE}.migrated.{timestamp}"));
    fs::rename(universal_path, &archive).with_context(|| {
        format!(
            "archive {} to {}",
            universal_path.display(),
            archive.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::domain::providers::model::{AppKind, Provider};
    use crate::domain::providers::store::providers_path;
    use serde_json::json;

    #[test]
    fn orphan_universal_derivatives_strips_link_and_category() {
        let mut store = ProviderStore::default();
        let mut extra = BTreeMap::new();
        extra.insert("universalProviderId".to_string(), json!("abc"));
        store.upsert(
            AppKind::Claude,
            Provider {
                id: "universal:abc:claude".to_string(),
                name: "gateway".to_string(),
                settings_config: json!({"env": {}}),
                category: Some("universal".to_string()),
                meta: None,
                extra,
            },
        );

        assert_eq!(orphan_universal_derivatives(&mut store), 1);
        let provider = &store.providers[0].provider;
        assert_eq!(provider.category.as_deref(), Some("custom"));
        assert!(!provider.extra.contains_key("universalProviderId"));
        assert_eq!(provider.settings_config["env"], json!({}));
    }

    #[test]
    fn migrate_is_idempotent() {
        let config_dir = std::env::temp_dir().join(format!(
            "cc-switch-universal-migrate-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join(UNIVERSAL_PROVIDERS_FILE),
            r#"{"providers":{}}"#,
        )
        .unwrap();
        fs::write(
            providers_path(&config_dir),
            r#"{"providers":[{"app":"claude","provider":{"id":"universal:u1:claude","name":"u","settingsConfig":{},"category":"universal","extra":{"universalProviderId":"u1"}},"providerType":"claude","providerTypeId":"claude"}]}"#,
        )
        .unwrap();

        assert!(migrate_remove_universal_layer(&config_dir).unwrap());
        assert!(!config_dir.join(UNIVERSAL_PROVIDERS_FILE).exists());
        assert!(config_dir.join(MIGRATION_MARKER).exists());
        assert!(!migrate_remove_universal_layer(&config_dir).unwrap());

        let _ = fs::remove_dir_all(config_dir);
    }
}
