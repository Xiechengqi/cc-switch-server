use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::core::usage::now_ms;

const BACKUPS_DIR_NAME: &str = "backups";
const BACKUP_MANIFEST_FILE_NAME: &str = "manifest.json";
const DEFAULT_BACKUP_KEEP: usize = 24;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupManifest {
    pub id: String,
    pub created_at_ms: u128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub files: Vec<BackupFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupFile {
    pub file_name: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupRestoreResult {
    pub restored: BackupManifest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_restore: Option<BackupManifest>,
}

pub fn create_backup(config_dir: &Path, reason: Option<String>) -> anyhow::Result<BackupManifest> {
    create_backup_inner(config_dir, reason, true)
}

fn create_backup_inner(
    config_dir: &Path,
    reason: Option<String>,
    prune_after_create: bool,
) -> anyhow::Result<BackupManifest> {
    fs::create_dir_all(backups_dir(config_dir))
        .with_context(|| format!("create backups dir {}", backups_dir(config_dir).display()))?;
    let id = generate_backup_id();
    let backup_dir = backup_dir(config_dir, &id)?;
    fs::create_dir_all(&backup_dir)
        .with_context(|| format!("create backup dir {}", backup_dir.display()))?;

    let mut files = Vec::new();
    for source in store_paths(config_dir) {
        if !source.exists() {
            continue;
        }
        let Some(file_name) = source.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let destination = backup_dir.join(file_name);
        fs::copy(&source, &destination).with_context(|| {
            format!(
                "copy backup file {} to {}",
                source.display(),
                destination.display()
            )
        })?;
        fs::File::open(&destination)
            .with_context(|| format!("open backup file {} for sync", destination.display()))?
            .sync_all()
            .with_context(|| format!("sync backup file {}", destination.display()))?;
        let size_bytes = fs::metadata(&destination)
            .with_context(|| format!("stat backup file {}", destination.display()))?
            .len();
        files.push(BackupFile {
            file_name: file_name.to_string(),
            size_bytes,
        });
    }

    let manifest = BackupManifest {
        id,
        created_at_ms: now_ms(),
        reason: reason.filter(|value| !value.trim().is_empty()),
        files,
    };
    crate::core::storage::write_json_pretty(&backup_dir.join(BACKUP_MANIFEST_FILE_NAME), &manifest)
        .with_context(|| format!("write backup manifest {}", manifest.id))?;
    if prune_after_create {
        prune_backups(config_dir, DEFAULT_BACKUP_KEEP)?;
    }
    Ok(manifest)
}

pub fn list_backups(config_dir: &Path) -> anyhow::Result<Vec<BackupManifest>> {
    let dir = backups_dir(config_dir);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut backups = Vec::new();
    for entry in
        fs::read_dir(&dir).with_context(|| format!("read backups dir {}", dir.display()))?
    {
        let entry = entry.with_context(|| format!("read backups entry {}", dir.display()))?;
        let manifest_path = entry.path().join(BACKUP_MANIFEST_FILE_NAME);
        if !manifest_path.exists() {
            continue;
        }
        let content = fs::read_to_string(&manifest_path)
            .with_context(|| format!("read backup manifest {}", manifest_path.display()))?;
        if let Ok(manifest) = serde_json::from_str::<BackupManifest>(&content) {
            backups.push(manifest);
        }
    }
    backups.sort_by(|left, right| {
        right
            .created_at_ms
            .cmp(&left.created_at_ms)
            .then(left.id.cmp(&right.id))
    });
    Ok(backups)
}

pub fn restore_backup(config_dir: &Path, backup_id: &str) -> anyhow::Result<BackupRestoreResult> {
    validate_backup_id(backup_id)?;
    let restored = read_manifest(config_dir, backup_id)?;
    let pre_restore =
        create_backup_inner(config_dir, Some(format!("pre-restore {backup_id}")), false).ok();
    let source_dir = backup_dir(config_dir, backup_id)?;
    for file in &restored.files {
        validate_backup_file_name(&file.file_name)?;
        let source = source_dir.join(&file.file_name);
        let destination = config_dir.join(&file.file_name);
        let content =
            fs::read(&source).with_context(|| format!("read backup file {}", source.display()))?;
        crate::core::storage::write_bytes_atomic(&destination, &content).with_context(|| {
            format!(
                "restore backup file {} to {}",
                source.display(),
                destination.display()
            )
        })?;
    }
    Ok(BackupRestoreResult {
        restored,
        pre_restore,
    })
}

pub fn prune_backups(config_dir: &Path, keep: usize) -> anyhow::Result<usize> {
    let backups = list_backups(config_dir)?;
    if backups.len() <= keep {
        return Ok(0);
    }
    let mut pruned = 0;
    for backup in backups.into_iter().skip(keep) {
        let dir = backup_dir(config_dir, &backup.id)?;
        if dir.exists() {
            fs::remove_dir_all(&dir)
                .with_context(|| format!("remove old backup {}", dir.display()))?;
            pruned += 1;
        }
    }
    Ok(pruned)
}

pub fn store_paths_for_export(config_dir: &Path) -> Vec<PathBuf> {
    store_paths(config_dir)
}

pub fn validate_export_file_name(value: &str) -> anyhow::Result<()> {
    validate_backup_file_name(value)
}

pub fn delete_backup(config_dir: &Path, backup_id: &str) -> anyhow::Result<()> {
    validate_backup_id(backup_id)?;
    let dir = backup_dir(config_dir, backup_id)?;
    if dir.exists() {
        fs::remove_dir_all(&dir).with_context(|| format!("remove backup {}", dir.display()))?;
    }
    Ok(())
}

pub fn rename_backup(
    config_dir: &Path,
    backup_id: &str,
    display_name: &str,
) -> anyhow::Result<BackupManifest> {
    validate_backup_id(backup_id)?;
    let trimmed = display_name.trim();
    if trimmed.is_empty() {
        anyhow::bail!("backup display name is required");
    }
    let mut manifest = read_manifest(config_dir, backup_id)?;
    manifest.reason = Some(format!("label:{trimmed}"));
    let path = backup_dir(config_dir, backup_id)?.join(BACKUP_MANIFEST_FILE_NAME);
    crate::core::storage::write_json_pretty(&path, &manifest)
        .with_context(|| format!("write backup manifest {}", path.display()))?;
    Ok(manifest)
}

pub fn backup_entry_for_frontend(manifest: &BackupManifest) -> Value {
    let size_bytes: u64 = manifest.files.iter().map(|file| file.size_bytes).sum();
    let created_at = manifest.created_at_ms.to_string();
    json!({
        "filename": manifest.id,
        "sizeBytes": size_bytes,
        "createdAt": created_at,
    })
}

pub fn backup_entries_for_frontend(manifests: &[BackupManifest]) -> Vec<Value> {
    manifests.iter().map(backup_entry_for_frontend).collect()
}

fn read_manifest(config_dir: &Path, backup_id: &str) -> anyhow::Result<BackupManifest> {
    validate_backup_id(backup_id)?;
    let path = backup_dir(config_dir, backup_id)?.join(BACKUP_MANIFEST_FILE_NAME);
    let content = fs::read_to_string(&path)
        .with_context(|| format!("read backup manifest {}", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("parse backup manifest {}", path.display()))
}

fn backups_dir(config_dir: &Path) -> PathBuf {
    config_dir.join(BACKUPS_DIR_NAME)
}

fn backup_dir(config_dir: &Path, backup_id: &str) -> anyhow::Result<PathBuf> {
    validate_backup_id(backup_id)?;
    Ok(backups_dir(config_dir).join(backup_id))
}

fn store_paths(config_dir: &Path) -> Vec<PathBuf> {
    vec![
        crate::core::config::config_path(config_dir),
        crate::core::email_auth::email_auth_path(config_dir),
        crate::core::providers::providers_path(config_dir),
        crate::core::universal_providers::universal_providers_path(config_dir),
        crate::core::accounts::accounts_path(config_dir),
        crate::core::failover::failover_path(config_dir),
        crate::core::pricing::model_pricing_path(config_dir),
        crate::core::usage::usage_path(config_dir),
        crate::core::shares::shares_path(config_dir),
        crate::core::tunnel::tunnels_path(config_dir),
    ]
}

fn generate_backup_id() -> String {
    let mut bytes = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut bytes);
    let suffix: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("backup-{}-{suffix}", now_ms())
}

fn validate_backup_id(value: &str) -> anyhow::Result<()> {
    let valid = !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_');
    if !valid {
        bail!("invalid backup id");
    }
    Ok(())
}

fn validate_backup_file_name(value: &str) -> anyhow::Result<()> {
    let valid = !value.is_empty()
        && !value.contains('/')
        && !value.contains('\\')
        && value.ends_with(".json");
    if !valid {
        bail!("invalid backup file name");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::*;

    #[test]
    fn backup_create_and_restore_round_trip_json_files() {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-server-backup-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        crate::core::storage::write_json_pretty(
            &crate::core::config::config_path(&dir),
            &json!({"owner": {"email": "before@example.com"}}),
        )
        .unwrap();
        let backup = create_backup(&dir, Some("test".to_string())).unwrap();
        crate::core::storage::write_json_pretty(
            &crate::core::config::config_path(&dir),
            &json!({"owner": {"email": "after@example.com"}}),
        )
        .unwrap();

        restore_backup(&dir, &backup.id).unwrap();

        let content = fs::read_to_string(crate::core::config::config_path(&dir)).unwrap();
        assert!(content.contains("before@example.com"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
