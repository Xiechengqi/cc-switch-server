use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::infra::time::now_ms;

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

pub fn create_backup(
    config_dir: &Path,
    targets: &[PathBuf],
    reason: Option<String>,
) -> anyhow::Result<BackupManifest> {
    create_backup_inner(config_dir, targets, reason, true)
}

fn create_backup_inner(
    config_dir: &Path,
    targets: &[PathBuf],
    reason: Option<String>,
    prune_after_create: bool,
) -> anyhow::Result<BackupManifest> {
    fs::create_dir_all(backups_dir(config_dir))
        .with_context(|| format!("create backups dir {}", backups_dir(config_dir).display()))?;
    set_private_directory_permissions(&backups_dir(config_dir))?;
    let id = generate_backup_id();
    let backup_dir = backup_dir(config_dir, &id)?;
    fs::create_dir_all(&backup_dir)
        .with_context(|| format!("create backup dir {}", backup_dir.display()))?;
    set_private_directory_permissions(&backup_dir)?;

    let mut files = Vec::new();
    for source in expand_backup_sources(targets)? {
        let relative = source.strip_prefix(config_dir).with_context(|| {
            format!(
                "backup source {} must be inside {}",
                source.display(),
                config_dir.display()
            )
        })?;
        let file_name = relative
            .to_str()
            .context("backup source path must be valid UTF-8")?;
        validate_backup_file_name(file_name)?;
        if is_legacy_token_market_archive_path(relative) {
            continue;
        }
        let destination = backup_dir.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create backup directory {}", parent.display()))?;
            set_private_directory_permissions(parent)?;
        }
        fs::copy(&source, &destination).with_context(|| {
            format!(
                "copy backup file {} to {}",
                source.display(),
                destination.display()
            )
        })?;
        set_private_file_permissions(&destination)?;
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
    crate::infra::storage::write_json_pretty(
        &backup_dir.join(BACKUP_MANIFEST_FILE_NAME),
        &manifest,
    )
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
    restore_backup_with_validator(config_dir, backup_id, |_, _| Ok(()))
}

pub fn restore_backup_with_validator(
    config_dir: &Path,
    backup_id: &str,
    validate: impl FnOnce(&Path, &BackupManifest) -> anyhow::Result<()>,
) -> anyhow::Result<BackupRestoreResult> {
    validate_backup_id(backup_id)?;
    let restored = restorable_manifest(read_manifest(config_dir, backup_id)?);
    let source_dir = backup_dir(config_dir, backup_id)?;
    let staged = stage_restore_files(&source_dir, &restored)?;
    validate_restore_stage(config_dir, &staged, &restored, validate)?;
    let pre_restore_targets = manifest_targets(config_dir, &restored)?;
    let pre_restore = create_backup_inner(
        config_dir,
        &pre_restore_targets,
        Some(format!("pre-restore {backup_id}")),
        false,
    )
    .ok();
    for directory in replacement_directories(config_dir, &restored)? {
        if directory.exists() {
            fs::remove_dir_all(&directory)
                .with_context(|| format!("replace restored directory {}", directory.display()))?;
        }
    }
    for (file, content) in staged {
        let destination = config_dir.join(&file.file_name);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create restore directory {}", parent.display()))?;
            set_private_directory_permissions(parent)?;
        }
        crate::infra::storage::write_bytes_atomic(&destination, &content).with_context(|| {
            format!(
                "restore backup file {} to {}",
                source_dir.join(&file.file_name).display(),
                destination.display()
            )
        })?;
    }
    Ok(BackupRestoreResult {
        restored,
        pre_restore,
    })
}

fn restorable_manifest(mut manifest: BackupManifest) -> BackupManifest {
    manifest
        .files
        .retain(|file| !is_legacy_token_market_archive_path(Path::new(&file.file_name)));
    manifest
}

fn stage_restore_files(
    source_dir: &Path,
    manifest: &BackupManifest,
) -> anyhow::Result<Vec<(BackupFile, Vec<u8>)>> {
    let mut staged = Vec::with_capacity(manifest.files.len());
    for file in &manifest.files {
        validate_backup_file_name(&file.file_name)?;
        let source = source_dir.join(&file.file_name);
        let content =
            fs::read(&source).with_context(|| format!("read backup file {}", source.display()))?;
        if content.len() as u64 != file.size_bytes {
            bail!(
                "backup file size mismatch for {}: manifest={}, actual={}",
                file.file_name,
                file.size_bytes,
                content.len()
            );
        }
        staged.push((file.clone(), content));
    }
    Ok(staged)
}

fn validate_restore_stage(
    config_dir: &Path,
    staged: &[(BackupFile, Vec<u8>)],
    manifest: &BackupManifest,
    validate: impl FnOnce(&Path, &BackupManifest) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let stage_dir =
        backups_dir(config_dir).join(format!(".restore-stage-{}", generate_backup_id()));
    fs::create_dir_all(&stage_dir)
        .with_context(|| format!("create restore stage {}", stage_dir.display()))?;
    set_private_directory_permissions(&stage_dir)?;
    let result = (|| {
        for (file, content) in staged {
            let destination = stage_dir.join(&file.file_name);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("create restore stage directory {}", parent.display())
                })?;
                set_private_directory_permissions(parent)?;
            }
            crate::infra::storage::write_bytes_atomic(&destination, content)
                .with_context(|| format!("stage backup file {}", file.file_name))?;
        }

        validate(&stage_dir, manifest)
    })();
    let cleanup = fs::remove_dir_all(&stage_dir)
        .with_context(|| format!("remove restore stage {}", stage_dir.display()));
    result.and(cleanup)
}

fn set_private_directory_permissions(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("chmod 0700 {}", path.display()))?;
    }
    Ok(())
}

fn set_private_file_permissions(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 0600 {}", path.display()))?;
    }
    Ok(())
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
    crate::infra::storage::write_json_pretty(&path, &manifest)
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

fn manifest_targets(config_dir: &Path, manifest: &BackupManifest) -> anyhow::Result<Vec<PathBuf>> {
    let mut targets = BTreeSet::new();
    for file in &manifest.files {
        validate_backup_file_name(&file.file_name)?;
        let path = Path::new(&file.file_name);
        if path.components().count() > 1 {
            let top = path
                .components()
                .next()
                .and_then(|component| match component {
                    Component::Normal(value) => Some(value),
                    _ => None,
                })
                .context("backup path has no top-level directory")?;
            targets.insert(config_dir.join(top));
        } else {
            targets.insert(config_dir.join(path));
        }
    }
    Ok(targets.into_iter().collect())
}

fn replacement_directories(
    config_dir: &Path,
    manifest: &BackupManifest,
) -> anyhow::Result<Vec<PathBuf>> {
    let targets = manifest_targets(config_dir, manifest)?;
    Ok(targets
        .into_iter()
        .filter(|path| path.file_name().and_then(|value| value.to_str()) == Some("usage"))
        .collect())
}

fn expand_backup_sources(targets: &[PathBuf]) -> anyhow::Result<Vec<PathBuf>> {
    fn collect(path: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error).with_context(|| format!("stat {}", path.display())),
        };
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "backup targets cannot be symlinks"
        );
        if metadata.is_file() {
            files.push(path.to_path_buf());
            return Ok(());
        }
        anyhow::ensure!(
            metadata.is_dir(),
            "backup target is not a file or directory"
        );
        let mut entries = fs::read_dir(path)
            .with_context(|| format!("read backup target directory {}", path.display()))?
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            collect(&entry.path(), files)?;
        }
        Ok(())
    }

    let mut files = Vec::new();
    for target in targets {
        collect(target, &mut files)?;
    }
    files.sort();
    files.dedup();
    Ok(files)
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
    let path = Path::new(value);
    let components = path.components().collect::<Vec<_>>();
    let normal_components = components
        .iter()
        .all(|component| matches!(component, Component::Normal(_)));
    let allowed_nested_path = components.len() <= 1
        || matches!(components.first(), Some(Component::Normal(value)) if *value == "usage")
        || is_legacy_token_market_archive_path(path);
    let valid = !value.is_empty()
        && !value.contains('\\')
        && !path.is_absolute()
        && normal_components
        && allowed_nested_path
        && (value.ends_with(".json") || value.ends_with(".jsonl") || value == "accounts.key");
    if !valid {
        bail!("invalid backup file name");
    }
    Ok(())
}

fn is_legacy_token_market_archive_path(path: &Path) -> bool {
    let components = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>();
    components.len() == 4
        && components[0] == "legacy-token-market-archive"
        && components[1] == "shares"
        && components[2].len() == 64
        && components[2].bytes().all(|byte| byte.is_ascii_hexdigit())
        && matches!(components[3], "shares.json" | "manifest.json")
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use base64::Engine;
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
        let config_path = dir.join("custom.json");
        crate::infra::storage::write_json_pretty(
            &config_path,
            &json!({"owner": {"email": "before@example.com"}}),
        )
        .unwrap();
        let backup = create_backup(
            &dir,
            std::slice::from_ref(&config_path),
            Some("test".to_string()),
        )
        .unwrap();
        crate::infra::storage::write_json_pretty(
            &config_path,
            &json!({"owner": {"email": "after@example.com"}}),
        )
        .unwrap();

        restore_backup(&dir, &backup.id).unwrap();

        let content = fs::read_to_string(config_path).unwrap();
        assert!(content.contains("before@example.com"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn backup_round_trips_usage_directory_without_leaving_stale_events() {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-server-backup-usage-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let usage = dir.join("usage");
        let events = usage.join("events");
        fs::create_dir_all(&events).unwrap();
        fs::write(usage.join("manifest.json"), br#"{"schemaVersion":1}"#).unwrap();
        fs::write(usage.join("requests.json"), br#"{"logs":["before"]}"#).unwrap();
        fs::write(events.join("2026-08-10.jsonl"), b"before\n").unwrap();

        let backup = create_backup(&dir, std::slice::from_ref(&usage), None).unwrap();
        assert!(backup
            .files
            .iter()
            .any(|file| file.file_name == "usage/events/2026-08-10.jsonl"));
        fs::write(usage.join("requests.json"), br#"{"logs":["after"]}"#).unwrap();
        fs::write(events.join("2026-08-11.jsonl"), b"stale\n").unwrap();

        restore_backup(&dir, &backup.id).unwrap();

        assert!(fs::read_to_string(usage.join("requests.json"))
            .unwrap()
            .contains("before"));
        assert!(events.join("2026-08-10.jsonl").exists());
        assert!(!events.join("2026-08-11.jsonl").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn backup_and_restore_exclude_retired_legacy_share_archive_payloads() {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-server-backup-legacy-share-archive-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let digest = "a".repeat(64);
        let archive_root = dir.join("legacy-token-market-archive");
        let entry = archive_root.join("shares").join(&digest);
        fs::create_dir_all(&entry).unwrap();
        fs::write(entry.join("shares.json"), br#"{"shares":[]}"#).unwrap();
        fs::write(
            entry.join("manifest.json"),
            format!(r#"{{"sourceSha256":"{digest}"}}"#),
        )
        .unwrap();
        let shares = dir.join("shares.json");
        fs::write(&shares, br#"{"shares":[]}"#).unwrap();

        let backup = create_backup(&dir, &[shares.clone(), archive_root.clone()], None).unwrap();
        assert_eq!(
            backup
                .files
                .iter()
                .map(|file| file.file_name.as_str())
                .collect::<Vec<_>>(),
            vec!["shares.json"]
        );

        let mut historical_manifest = backup.clone();
        historical_manifest.files.push(BackupFile {
            file_name: format!("legacy-token-market-archive/shares/{digest}/shares.json"),
            size_bytes: br#"{"shares":[]}"#.len() as u64,
        });
        assert_eq!(restorable_manifest(historical_manifest).files.len(), 1);

        fs::remove_dir_all(&archive_root).unwrap();
        fs::write(&shares, br#"{"shares":[{"id":"new"}]}"#).unwrap();

        restore_backup(&dir, &backup.id).unwrap();

        assert_eq!(fs::read(&shares).unwrap(), br#"{"shares":[]}"#);
        assert!(!archive_root.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn restore_validator_failure_prevents_live_file_replacement() {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-server-backup-provider-stage-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("providers.json");
        let live = br#"{"providers":[]}"#;
        fs::write(&path, live).unwrap();
        let backup = create_backup(&dir, std::slice::from_ref(&path), None).unwrap();
        let backup_file = backup_dir(&dir, &backup.id).unwrap().join("providers.json");
        let malformed = br#"{"providers":"bad"}"#;
        fs::write(&backup_file, malformed).unwrap();
        let mut manifest = read_manifest(&dir, &backup.id).unwrap();
        manifest.files[0].size_bytes = malformed.len() as u64;
        crate::infra::storage::write_json_pretty(
            &backup_dir(&dir, &backup.id)
                .unwrap()
                .join(BACKUP_MANIFEST_FILE_NAME),
            &manifest,
        )
        .unwrap();

        let error = restore_backup_with_validator(&dir, &backup.id, |stage_dir, _| {
            let value: Value =
                serde_json::from_slice(&fs::read(stage_dir.join("providers.json"))?)?;
            if !value.get("providers").is_some_and(Value::is_array) {
                anyhow::bail!("validate staged providers.json");
            }
            Ok(())
        })
        .unwrap_err();

        assert!(error.to_string().contains("validate staged providers.json"));
        assert_eq!(fs::read(&path).unwrap(), live);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn restore_validator_can_reject_multiple_store_schemas() {
        assert_malformed_store_is_rejected(
            "email-auth.json",
            br#"{"email":"owner@example.com","verifiedAt":1}"#,
            br#"{"invalid":"shape"}"#,
            "validate staged email-auth.json",
        );
        assert_malformed_store_is_rejected(
            "tunnels.json",
            br#"{"statuses":{}}"#,
            br#"{"statuses":"bad"}"#,
            "validate staged tunnels.json",
        );
    }

    fn assert_malformed_store_is_rejected(
        file_name: &str,
        live: &[u8],
        malformed: &[u8],
        expected_error: &str,
    ) {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-server-backup-schema-stage-test-{}-{}",
            file_name,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(file_name);
        fs::write(&path, live).unwrap();
        let backup = create_backup(&dir, std::slice::from_ref(&path), None).unwrap();
        let backup_file = backup_dir(&dir, &backup.id).unwrap().join(file_name);
        fs::write(&backup_file, malformed).unwrap();
        let mut manifest = read_manifest(&dir, &backup.id).unwrap();
        manifest.files[0].size_bytes = malformed.len() as u64;
        crate::infra::storage::write_json_pretty(
            &backup_dir(&dir, &backup.id)
                .unwrap()
                .join(BACKUP_MANIFEST_FILE_NAME),
            &manifest,
        )
        .unwrap();

        let error = restore_backup_with_validator(&dir, &backup.id, |stage_dir, _| {
            let value: Value = serde_json::from_slice(&fs::read(stage_dir.join(file_name))?)?;
            let valid = match file_name {
                "email-auth.json" => value.get("email").is_some_and(Value::is_string),
                "tunnels.json" => value.get("statuses").is_some_and(Value::is_object),
                _ => false,
            };
            if !valid {
                anyhow::bail!(expected_error.to_string());
            }
            Ok(())
        })
        .unwrap_err();

        assert!(error.to_string().contains(expected_error), "{error:#}");
        assert_eq!(fs::read(&path).unwrap(), live);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn account_key_is_restored() {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-server-backup-account-key-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("accounts.key");
        let before = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7_u8; 32]);
        fs::write(&path, format!("{before}\n")).unwrap();
        let backup = create_backup(&dir, std::slice::from_ref(&path), None).unwrap();
        let after = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([8_u8; 32]);
        fs::write(&path, format!("{after}\n")).unwrap();

        restore_backup(&dir, &backup.id).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap().trim(), before);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn backup_files_and_directories_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "cc-switch-server-backup-permissions-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let source = dir.join("custom.json");
        fs::write(&source, b"{}").unwrap();

        let backup = create_backup(&dir, std::slice::from_ref(&source), None).unwrap();
        let root = backups_dir(&dir);
        let backup_path = backup_dir(&dir, &backup.id).unwrap();

        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&backup_path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(backup_path.join("custom.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
