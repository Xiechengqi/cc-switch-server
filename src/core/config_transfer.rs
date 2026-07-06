use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::Context;
use serde_json::{json, Value};

use crate::core::backup::{create_backup, restore_backup};

pub fn export_config_bundle(config_dir: &Path) -> anyhow::Result<Value> {
    let mut files = BTreeMap::new();
    for source in crate::core::backup::store_paths_for_export(config_dir) {
        if !source.exists() {
            continue;
        }
        let Some(file_name) = source.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let content = fs::read_to_string(&source)
            .with_context(|| format!("read config file {}", source.display()))?;
        let parsed: Value = serde_json::from_str(&content)
            .with_context(|| format!("parse config file {}", source.display()))?;
        files.insert(file_name.to_string(), parsed);
    }
    Ok(json!({
        "version": 1,
        "format": "cc-switch-server-config-bundle",
        "files": files,
    }))
}

pub fn import_config_bundle(config_dir: &Path, bundle: &Value) -> anyhow::Result<String> {
    let files = bundle
        .get("files")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("config bundle missing files object"))?;
    if files.is_empty() {
        anyhow::bail!("config bundle is empty");
    }

    let pre_backup = create_backup(config_dir, Some("pre-import".to_string()))?;
    for (file_name, content) in files {
        crate::core::backup::validate_export_file_name(file_name)?;
        let destination = config_dir.join(file_name);
        crate::core::storage::write_json_pretty(&destination, content)
            .with_context(|| format!("write imported config file {}", destination.display()))?;
    }
    Ok(pre_backup.id)
}

pub fn import_config_bundle_from_base64(
    config_dir: &Path,
    encoded: &str,
) -> anyhow::Result<String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .context("decode config bundle base64")?;
    let bundle: Value = serde_json::from_slice(&bytes).context("parse config bundle json")?;
    import_config_bundle(config_dir, &bundle)
}
