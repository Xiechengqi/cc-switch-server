use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::domain::accounts::store::AccountStore;
use crate::infra::credentials::{
    load_root_key_if_present, CredentialKeySource, ResolvedCredentialKey,
};

use super::credentials::{provider_credential_slot_is_supported, split_provider_credentials};
use super::model::AppKind;
use super::registry::ProviderKey;
use super::runtime::ProviderRuntimePlan;
use super::store::{providers_path, ProviderStore, ProviderStoreFormat};

const MIGRATION_DIRECTORY: &str = "provider-migrations/s1-to-s2";
const SNAPSHOT_FILE: &str = "providers.s1.snapshot.json";
const MANIFEST_FILE: &str = "manifest.json";
const MANIFEST_FORMAT: &str = "cc-switch-provider-s1-to-s2-migration";
const MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStorageMigrationItemStatus {
    Ready,
    Blocked,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStorageMigrationItem {
    pub app: AppKind,
    pub provider_id: String,
    pub status: ProviderStorageMigrationItemStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocker_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStorageMigrationReport {
    pub source_format: ProviderStoreFormat,
    pub target_format: ProviderStoreFormat,
    pub key_source: String,
    pub provider_count: usize,
    pub ready_count: usize,
    pub blocked_count: usize,
    pub runtime_plan_parity: bool,
    pub reference_fingerprint: String,
    pub can_apply: bool,
    pub items: Vec<ProviderStorageMigrationItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStorageMigrationOutcome {
    pub ok: bool,
    pub action: String,
    pub changed: bool,
    pub report: ProviderStorageMigrationReport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_directory: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MigrationManifestStatus {
    Prepared,
    Applied,
    RolledBack,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MigrationManifest {
    format: String,
    version: u32,
    status: MigrationManifestStatus,
    source_sha256: String,
    source_bytes: u64,
    key_source: String,
    reference_sha256: BTreeMap<String, String>,
    created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    applied_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rolled_back_at: Option<String>,
}

pub fn preflight(config_dir: &Path) -> anyhow::Result<ProviderStorageMigrationReport> {
    let path = providers_path(config_dir);
    if path.exists() {
        let value: serde_json::Value = serde_json::from_slice(
            &fs::read(&path).with_context(|| format!("read {}", path.display()))?,
        )
        .with_context(|| format!("parse {}", path.display()))?;
        if super::store_v2::looks_like_s2(&value) {
            let mut store = ProviderStore::load_or_default(config_dir)?;
            let accounts = AccountStore::load_or_default(config_dir)?;
            store.rebuild_runtime_index(&accounts)?;
            return Ok(ProviderStorageMigrationReport {
                source_format: ProviderStoreFormat::S2,
                target_format: ProviderStoreFormat::S2,
                key_source: key_source_label(store.storage_status().credential_key_source, false),
                provider_count: store.providers.len(),
                ready_count: store.providers.len(),
                blocked_count: 0,
                runtime_plan_parity: true,
                reference_fingerprint: short_reference_fingerprint(config_dir)?,
                can_apply: false,
                items: store
                    .providers
                    .iter()
                    .map(|stored| ProviderStorageMigrationItem {
                        app: stored.app,
                        provider_id: stored.provider.id.clone(),
                        status: ProviderStorageMigrationItemStatus::Ready,
                        blocker_codes: Vec::new(),
                    })
                    .collect(),
            });
        }
    }

    let mut source = ProviderStore::load_or_default(config_dir)?;
    source.prepare_legacy_runtime_view();
    let accounts = AccountStore::load_or_default(config_dir)?;
    source.validate_for_commit()?;
    source.rebuild_runtime_index(&accounts)?;

    let mut items = Vec::with_capacity(source.providers.len());
    for stored in &source.providers {
        let mut blockers = Vec::new();
        if stored
            .resource
            .profile_id
            .as_ref()
            .is_none_or(|profile_id| {
                super::registry::profile_by_id(profile_id.as_str()).is_some_and(|profile| {
                    matches!(
                        &profile.driver_binding,
                        super::registry::DriverBinding::Fixed { driver_id }
                            if driver_id.as_str() == "legacy.frozen"
                    )
                })
            })
        {
            blockers.push("legacy_provider_requires_identity_resolution".to_string());
        }
        match split_provider_credentials(&stored.provider) {
            Ok((_, credentials)) => {
                if credentials
                    .keys()
                    .any(|slot| !provider_credential_slot_is_supported(slot))
                {
                    blockers.push("unsupported_credential_slot".to_string());
                }
            }
            Err(_) => blockers.push("credential_classification_failed".to_string()),
        }
        blockers.sort();
        blockers.dedup();
        items.push(ProviderStorageMigrationItem {
            app: stored.app,
            provider_id: stored.provider.id.clone(),
            status: if blockers.is_empty() {
                ProviderStorageMigrationItemStatus::Ready
            } else {
                ProviderStorageMigrationItemStatus::Blocked
            },
            blocker_codes: blockers,
        });
    }

    let existing_key = load_root_key_if_present(config_dir)?;
    let would_create_file_key = existing_key.is_none();
    let resolved = existing_key.unwrap_or_else(ephemeral_preflight_key);
    let mut candidate = source.clone();
    super::store_v2::seal_store(&mut candidate, resolved.clone())?;
    candidate.format = ProviderStoreFormat::S2;
    candidate.store_generation = 1;
    let encoded = super::store_v2::encode_s2(&candidate)?;
    let mut roundtrip = super::store_v2::decode_s2(encoded, resolved)?;
    roundtrip.rebuild_runtime_index(&accounts)?;
    let runtime_plan_parity = runtime_plans(&source) == runtime_plans(&roundtrip);

    let blocked_count = items
        .iter()
        .filter(|item| item.status == ProviderStorageMigrationItemStatus::Blocked)
        .count();
    let ready_count = items.len().saturating_sub(blocked_count);
    Ok(ProviderStorageMigrationReport {
        source_format: ProviderStoreFormat::S1,
        target_format: ProviderStoreFormat::S2,
        key_source: key_source_label(
            candidate.storage_status().credential_key_source,
            would_create_file_key,
        ),
        provider_count: items.len(),
        ready_count,
        blocked_count,
        runtime_plan_parity,
        reference_fingerprint: short_reference_fingerprint(config_dir)?,
        can_apply: blocked_count == 0 && runtime_plan_parity,
        items,
    })
}

pub fn apply(config_dir: &Path) -> anyhow::Result<ProviderStorageMigrationOutcome> {
    let _data_directory_lock = crate::infra::storage::acquire_data_directory_lock(config_dir)?;
    let report = preflight(config_dir)?;
    if report.source_format == ProviderStoreFormat::S2 {
        finish_interrupted_manifest_if_needed(config_dir)?;
        return Ok(ProviderStorageMigrationOutcome {
            ok: true,
            action: "apply".to_string(),
            changed: false,
            report,
            snapshot_directory: existing_snapshot_directory(config_dir),
        });
    }
    if !report.can_apply {
        anyhow::bail!(
            "Provider S2 migration is blocked ({} Provider blockers, runtime parity: {})",
            report.blocked_count,
            report.runtime_plan_parity
        );
    }

    let source = source_s1_bytes(config_dir)?;
    let source_sha256 = sha256_hex(&source);
    let migration_dir = migration_dir(config_dir);
    fs::create_dir_all(&migration_dir)
        .with_context(|| format!("create {}", migration_dir.display()))?;
    secure_directory(&migration_dir)?;
    let snapshot_path = migration_dir.join(SNAPSHOT_FILE);
    let manifest_path = migration_dir.join(MANIFEST_FILE);

    if snapshot_path.exists() || manifest_path.exists() {
        let manifest = load_manifest(&manifest_path)?;
        let snapshot = fs::read(&snapshot_path)
            .with_context(|| format!("read {}", snapshot_path.display()))?;
        if sha256_hex(&snapshot) != manifest.source_sha256
            || manifest.source_sha256 != source_sha256
        {
            anyhow::bail!("existing Provider migration snapshot does not match live S1 data");
        }
    } else {
        crate::infra::storage::write_bytes_atomic(&snapshot_path, &source)
            .context("write Provider S1 migration snapshot")?;
        let key = load_root_key_if_present(config_dir)?;
        let manifest = MigrationManifest {
            format: MANIFEST_FORMAT.to_string(),
            version: MANIFEST_VERSION,
            status: MigrationManifestStatus::Prepared,
            source_sha256,
            source_bytes: source.len() as u64,
            key_source: key_source_label(key.as_ref().map(|key| key.source), key.is_none()),
            reference_sha256: reference_hashes(config_dir)?,
            created_at: chrono::Utc::now().to_rfc3339(),
            applied_at: None,
            rolled_back_at: None,
        };
        write_manifest(&manifest_path, &manifest)?;
    }

    let mut store = ProviderStore::load_or_default(config_dir)?;
    store.prepare_legacy_runtime_view();
    store.promote_to_s2(config_dir)?;
    store.save(config_dir)?;

    let accounts = AccountStore::load_or_default(config_dir)?;
    let mut verified = ProviderStore::load_or_default(config_dir)
        .context("reload Provider S2 store after migration")?;
    verified
        .rebuild_runtime_index(&accounts)
        .context("compile Provider S2 runtime after migration")?;

    let mut manifest = load_manifest(&manifest_path)?;
    manifest.status = MigrationManifestStatus::Applied;
    manifest.applied_at = Some(chrono::Utc::now().to_rfc3339());
    write_manifest(&manifest_path, &manifest)?;
    Ok(ProviderStorageMigrationOutcome {
        ok: true,
        action: "apply".to_string(),
        changed: true,
        report,
        snapshot_directory: Some(migration_dir.display().to_string()),
    })
}

pub fn rollback(config_dir: &Path) -> anyhow::Result<ProviderStorageMigrationOutcome> {
    let _data_directory_lock = crate::infra::storage::acquire_data_directory_lock(config_dir)?;
    let migration_dir = migration_dir(config_dir);
    let snapshot_path = migration_dir.join(SNAPSHOT_FILE);
    let manifest_path = migration_dir.join(MANIFEST_FILE);
    let mut manifest = load_manifest(&manifest_path)?;
    let snapshot =
        fs::read(&snapshot_path).with_context(|| format!("read {}", snapshot_path.display()))?;
    if sha256_hex(&snapshot) != manifest.source_sha256
        || snapshot.len() as u64 != manifest.source_bytes
    {
        anyhow::bail!("Provider S1 migration snapshot failed integrity validation");
    }
    let _: ProviderStore = serde_json::from_slice(&snapshot)
        .context("Provider migration snapshot is not a valid S1 store")?;
    crate::infra::storage::write_bytes_atomic(&providers_path(config_dir), &snapshot)
        .context("restore Provider S1 migration snapshot")?;
    ProviderStore::load_or_default(config_dir).context("validate restored Provider S1 store")?;

    manifest.status = MigrationManifestStatus::RolledBack;
    manifest.rolled_back_at = Some(chrono::Utc::now().to_rfc3339());
    write_manifest(&manifest_path, &manifest)?;
    let report = preflight(config_dir)?;
    Ok(ProviderStorageMigrationOutcome {
        ok: true,
        action: "rollback".to_string(),
        changed: true,
        report,
        snapshot_directory: Some(migration_dir.display().to_string()),
    })
}

pub fn cleanup_snapshot(config_dir: &Path) -> anyhow::Result<bool> {
    let _data_directory_lock = crate::infra::storage::acquire_data_directory_lock(config_dir)?;
    let migration_dir = migration_dir(config_dir);
    if !migration_dir.exists() {
        return Ok(false);
    }
    let manifest = load_manifest(&migration_dir.join(MANIFEST_FILE))?;
    if manifest.status == MigrationManifestStatus::Prepared {
        anyhow::bail!("cannot clean up a prepared Provider migration snapshot");
    }
    fs::remove_dir_all(&migration_dir)
        .with_context(|| format!("remove {}", migration_dir.display()))?;
    if let Some(parent) = migration_dir.parent() {
        let _ = fs::File::open(parent).and_then(|directory| directory.sync_all());
    }
    Ok(true)
}

fn runtime_plans(store: &ProviderStore) -> BTreeMap<ProviderKey, ProviderRuntimePlan> {
    store
        .providers
        .iter()
        .filter_map(|stored| {
            store
                .runtime_plan(stored.app, &stored.provider.id)
                .map(|plan| (plan.provider_key.clone(), plan.as_ref().clone()))
        })
        .collect()
}

fn ephemeral_preflight_key() -> ResolvedCredentialKey {
    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    ResolvedCredentialKey {
        key,
        source: CredentialKeySource::File,
    }
}

fn key_source_label(source: Option<CredentialKeySource>, would_create_file: bool) -> String {
    if would_create_file {
        return "file_will_be_created".to_string();
    }
    match source {
        Some(CredentialKeySource::Environment) => "environment".to_string(),
        Some(CredentialKeySource::File) => "file".to_string(),
        None => "unavailable".to_string(),
    }
}

fn source_s1_bytes(config_dir: &Path) -> anyhow::Result<Vec<u8>> {
    let path = providers_path(config_dir);
    if path.exists() {
        return fs::read(&path).with_context(|| format!("read {}", path.display()));
    }
    Ok(b"{\n  \"providers\": []\n}\n".to_vec())
}

fn migration_dir(config_dir: &Path) -> PathBuf {
    config_dir.join(MIGRATION_DIRECTORY)
}

fn existing_snapshot_directory(config_dir: &Path) -> Option<String> {
    let path = migration_dir(config_dir);
    path.exists().then(|| path.display().to_string())
}

fn secure_directory(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("chmod 0700 {}", path.display()))?;
    }
    Ok(())
}

fn load_manifest(path: &Path) -> anyhow::Result<MigrationManifest> {
    let manifest: MigrationManifest = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("parse {}", path.display()))?;
    if manifest.format != MANIFEST_FORMAT || manifest.version != MANIFEST_VERSION {
        anyhow::bail!("unsupported Provider migration manifest");
    }
    Ok(manifest)
}

fn write_manifest(path: &Path, manifest: &MigrationManifest) -> anyhow::Result<()> {
    crate::infra::storage::write_json_pretty(path, manifest)
        .with_context(|| format!("write {}", path.display()))
}

fn finish_interrupted_manifest_if_needed(config_dir: &Path) -> anyhow::Result<()> {
    let path = migration_dir(config_dir).join(MANIFEST_FILE);
    if !path.exists() {
        return Ok(());
    }
    let mut manifest = load_manifest(&path)?;
    if manifest.status == MigrationManifestStatus::Prepared {
        manifest.status = MigrationManifestStatus::Applied;
        manifest.applied_at = Some(chrono::Utc::now().to_rfc3339());
        write_manifest(&path, &manifest)?;
    }
    Ok(())
}

fn reference_hashes(config_dir: &Path) -> anyhow::Result<BTreeMap<String, String>> {
    let mut hashes = BTreeMap::new();
    for file_name in ["accounts.json", "shares.json", "ui-settings.json"] {
        let path = config_dir.join(file_name);
        if path.exists() {
            hashes.insert(
                file_name.to_string(),
                sha256_hex(&fs::read(&path).with_context(|| format!("read {}", path.display()))?),
            );
        }
    }
    Ok(hashes)
}

fn short_reference_fingerprint(config_dir: &Path) -> anyhow::Result<String> {
    let hashes = reference_hashes(config_dir)?;
    let digest = Sha256::digest(serde_json::to_vec(&hashes)?);
    Ok(hex::encode(&digest[..8]))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::*;
    use crate::domain::providers::model::{Provider, ProviderType};
    use crate::domain::providers::registry::ProfileId;
    use crate::domain::providers::store::{ProviderResourceMetadata, StoredProvider};

    fn test_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "cc-switch-provider-storage-migration-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_s1(config_dir: &Path) {
        let store = ProviderStore {
            providers: vec![StoredProvider {
                app: AppKind::Codex,
                provider: Provider {
                    id: "provider-1".to_string(),
                    name: "OpenRouter".to_string(),
                    settings_config: json!({
                        "auth": {"OPENAI_API_KEY": "plaintext-secret"},
                        "base_url": "https://openrouter.ai/api/v1",
                        "modelMapping": {"mode": "single", "upstreamModel": "openai/gpt-5"}
                    }),
                    category: None,
                    meta: Some(super::super::model::ProviderMeta {
                        provider_type: Some("openrouter".to_string()),
                        ..Default::default()
                    }),
                    extra: Default::default(),
                },
                provider_type: ProviderType::OpenRouter,
                provider_type_id: "openrouter".to_string(),
                resource: ProviderResourceMetadata {
                    profile_id: Some(ProfileId::parse("codex.openrouter").unwrap()),
                    profile_schema_revision: Some(1),
                    revision: 1,
                    credential_generation: 1,
                    ..Default::default()
                },
            }],
            ..Default::default()
        };
        store.save(config_dir).unwrap();
    }

    #[test]
    fn preflight_is_read_only_and_apply_can_rollback() {
        let config_dir = test_dir("apply-rollback");
        write_s1(&config_dir);
        let before = fs::read(providers_path(&config_dir)).unwrap();
        let report = preflight(&config_dir).unwrap();
        assert!(report.can_apply);
        assert_eq!(before, fs::read(providers_path(&config_dir)).unwrap());
        assert!(!config_dir.join("accounts.key").exists());

        let outcome = apply(&config_dir).unwrap();
        assert!(outcome.changed);
        let s2 = fs::read_to_string(providers_path(&config_dir)).unwrap();
        assert!(s2.contains(super::super::store_v2::PROVIDER_STORE_GUARD));
        assert!(!s2.contains("plaintext-secret"));
        assert!(config_dir.join("accounts.key").exists());

        let rollback_outcome = rollback(&config_dir).unwrap();
        assert!(rollback_outcome.changed);
        assert_eq!(before, fs::read(providers_path(&config_dir)).unwrap());
        fs::remove_dir_all(config_dir).unwrap();
    }

    #[test]
    fn legacy_provider_blocks_store_cutover() {
        let config_dir = test_dir("legacy-blocked");
        let mut store = ProviderStore::default();
        store.upsert(
            AppKind::Codex,
            Provider {
                id: "legacy".to_string(),
                name: "Legacy".to_string(),
                settings_config: json!({}),
                category: None,
                meta: None,
                extra: Default::default(),
            },
        );
        store.save(&config_dir).unwrap();
        let report = preflight(&config_dir).unwrap();
        assert!(!report.can_apply);
        assert_eq!(report.blocked_count, 1);
        fs::remove_dir_all(config_dir).unwrap();
    }

    #[test]
    fn apply_is_rejected_while_server_holds_data_directory_lock() {
        let config_dir = test_dir("process-lock");
        write_s1(&config_dir);
        let before = fs::read(providers_path(&config_dir)).unwrap();
        let _server_lock = crate::infra::storage::acquire_data_directory_lock(&config_dir).unwrap();

        let error = apply(&config_dir).unwrap_err();

        assert!(error.to_string().contains("already in use"), "{error:#}");
        assert_eq!(fs::read(providers_path(&config_dir)).unwrap(), before);
        assert!(!migration_dir(&config_dir).exists());
        fs::remove_dir_all(config_dir).unwrap();
    }
}
