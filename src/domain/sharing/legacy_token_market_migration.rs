use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::retired_fields::RETIRED_SHARE_FIELDS;
use super::shares::{shares_path, ShareStore};

const RETIRED_ARCHIVE_DIRECTORY: &str = "legacy-token-market-archive";
const RETIREMENT_AUDIT_FILE: &str = "data-retirement-audit.json";
const AUDIT_FORMAT: &str = "cc-switch-server-data-retirement-audit";
const AUDIT_VERSION: u32 = 1;
const RETIRED_COMPONENT: &str = "share-contract-v1-token-market-fields";

#[derive(Debug, Clone)]
pub(crate) struct LegacyTokenMarketLoad {
    pub store: ShareStore,
    pub migration: Option<LegacyTokenMarketMigration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LegacyTokenMarketMigration {
    pub source_sha256: Option<String>,
    pub affected_fields: usize,
    pub retired_archive_files: usize,
    pub audit_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DataRetirementAudit {
    format: String,
    version: u32,
    component: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_sha256: Option<String>,
    affected_fields: usize,
    retired_archive_files: usize,
    retired_at_ms: u128,
}

/// Load `shares.json`, migrate the final useful ShareTo identities to
/// canonical `userGrants`, and remove every retired Token Market/ACL field.
///
/// The cleaned file is atomically replaced and re-read before any historical
/// archive is removed. No new snapshot of the source bytes or email value is
/// created: the only retained receipt is a non-identifying hash/count audit.
pub(crate) fn load_and_migrate(config_dir: &Path) -> anyhow::Result<LegacyTokenMarketLoad> {
    let path = shares_path(config_dir);
    let mut source_sha256 = None;
    let mut affected_fields = 0;

    let store = if path.exists() {
        let source = fs::read(&path).with_context(|| format!("read shares {}", path.display()))?;
        let mut cleaned_value: Value = serde_json::from_slice(&source)
            .with_context(|| format!("parse shares {}", path.display()))?;
        affected_fields = migrate_legacy_share_contract(&mut cleaned_value)?;
        let store: ShareStore = serde_json::from_value(cleaned_value.clone())
            .with_context(|| format!("decode shares {}", path.display()))?;

        if affected_fields != 0 {
            source_sha256 = Some(sha256_hex(&source));
            let mut cleaned = serde_json::to_vec_pretty(&cleaned_value)
                .context("serialize shares after retiring legacy capacity binding")?;
            cleaned.push(b'\n');
            crate::infra::storage::write_bytes_atomic(&path, &cleaned).with_context(|| {
                format!("retire legacy capacity binding from {}", path.display())
            })?;

            let persisted = fs::read(&path)
                .with_context(|| format!("verify migrated shares {}", path.display()))?;
            ensure!(
                sha256_hex(&persisted) == sha256_hex(&cleaned),
                "migrated shares checksum mismatch"
            );
            let mut persisted_value: Value = serde_json::from_slice(&persisted)
                .with_context(|| format!("parse migrated shares {}", path.display()))?;
            ensure!(
                migrate_legacy_share_contract(&mut persisted_value)? == 0,
                "migrated shares still contain retired Share contract fields"
            );
        }
        store
    } else {
        ShareStore::default()
    };

    // Count and validate the archive before writing the receipt.  The receipt
    // is durable before deletion so an I/O failure cannot leave the process
    // with deleted payloads and no record of what was retired.  A retry can
    // safely finish removing the archive using the existing receipt.
    let retired_archive_files = count_retired_archive_payload(config_dir)
        .context("inspect retired capacity-market Share archive")?;
    if affected_fields == 0 && retired_archive_files == 0 {
        return Ok(LegacyTokenMarketLoad {
            store,
            migration: None,
        });
    }

    let audit_path = retirement_audit_path(config_dir);
    let previous_audit = read_retirement_audit(&audit_path)?;
    let audit = DataRetirementAudit {
        format: AUDIT_FORMAT.to_string(),
        version: AUDIT_VERSION,
        component: RETIRED_COMPONENT.to_string(),
        source_sha256: source_sha256.clone().or_else(|| {
            previous_audit
                .as_ref()
                .and_then(|item| item.source_sha256.clone())
        }),
        affected_fields: affected_fields.max(
            previous_audit
                .as_ref()
                .map(|item| item.affected_fields)
                .unwrap_or_default(),
        ),
        retired_archive_files: retired_archive_files.max(
            previous_audit
                .as_ref()
                .map(|item| item.retired_archive_files)
                .unwrap_or_default(),
        ),
        retired_at_ms: previous_audit
            .as_ref()
            .map(|item| item.retired_at_ms)
            .unwrap_or_else(crate::infra::time::now_ms),
    };
    crate::infra::storage::write_json_pretty(&audit_path, &audit)
        .with_context(|| format!("write data-retirement audit {}", audit_path.display()))?;

    if archive_root(config_dir).exists() {
        let removed = remove_retired_archive_payload(config_dir)
            .context("remove retired capacity-market Share archive")?;
        ensure!(
            removed == retired_archive_files,
            "retired archive file count changed while removing payload"
        );
    }

    Ok(LegacyTokenMarketLoad {
        store,
        migration: Some(LegacyTokenMarketMigration {
            source_sha256: audit.source_sha256.clone(),
            affected_fields: audit.affected_fields,
            retired_archive_files: audit.retired_archive_files,
            audit_path,
        }),
    })
}

pub(crate) fn archive_root(config_dir: &Path) -> PathBuf {
    config_dir.join(RETIRED_ARCHIVE_DIRECTORY)
}

pub(crate) fn retirement_audit_path(config_dir: &Path) -> PathBuf {
    config_dir.join(RETIREMENT_AUDIT_FILE)
}

pub(super) fn migrate_legacy_share_contract(value: &mut Value) -> anyhow::Result<usize> {
    let Some(shares) = value.get_mut("shares") else {
        return Ok(0);
    };
    let shares = shares
        .as_array_mut()
        .context("shares.json field `shares` must be an array")?;
    let mut changed = 0;
    for share in shares {
        let share = share
            .as_object_mut()
            .context("each shares.json Share must be an object")?;
        let canonical_grants_empty = match share.get("userGrants") {
            None | Some(Value::Null) => true,
            Some(Value::Object(grants)) => grants.is_empty(),
            Some(_) => anyhow::bail!("Share `userGrants` must be an object"),
        };
        let mut legacy_emails = BTreeSet::new();
        if canonical_grants_empty {
            collect_legacy_shareto_emails(share, &mut legacy_emails)?;
        }

        let owner = share
            .get("ownerEmail")
            .and_then(Value::as_str)
            .map(normalize_legacy_email)
            .filter(|email| !email.is_empty());
        legacy_emails.retain(|email| owner.as_deref() != Some(email.as_str()));

        if share.get("freeAccess").is_none() {
            let free = share
                .get("forSale")
                .or_else(|| share.get("for_sale"))
                .and_then(Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case("free"));
            if free {
                share.insert("freeAccess".to_string(), Value::Bool(true));
                changed += 1;
            }
        }

        for field in RETIRED_SHARE_FIELDS {
            if share.remove(*field).is_some() {
                changed += 1;
            }
        }

        // `runtimeSnapshot` and other forward-compatible objects are raw
        // JSON.  Scrub retired keys recursively so ignored fields cannot
        // survive in the persisted file or be reintroduced by a later
        // round-trip through an older UI snapshot.
        for value in share.values_mut() {
            changed += scrub_retired_share_fields(value);
        }

        if canonical_grants_empty && !legacy_emails.is_empty() {
            let policy = legacy_user_policy(share);
            let now = crate::infra::time::now_ms();
            let grants = legacy_emails
                .into_iter()
                .map(|email| {
                    let grant = json!({
                        "email": email,
                        "role": "shareto",
                        "active": true,
                        "policy": policy,
                        "createdAtMs": now,
                        "updatedAtMs": now,
                        "revision": 1,
                        "manager": "manual"
                    });
                    (email, grant)
                })
                .collect::<Map<String, Value>>();
            share.insert("userGrants".to_string(), Value::Object(grants));
            changed += 1;
        }
    }
    Ok(changed)
}

fn scrub_retired_share_fields(value: &mut Value) -> usize {
    match value {
        Value::Object(object) => {
            let mut changed = 0;
            for field in RETIRED_SHARE_FIELDS {
                if object.remove(*field).is_some() {
                    changed += 1;
                }
            }
            for child in object.values_mut() {
                changed += scrub_retired_share_fields(child);
            }
            changed
        }
        Value::Array(values) => values.iter_mut().map(scrub_retired_share_fields).sum(),
        _ => 0,
    }
}

fn collect_legacy_shareto_emails(
    share: &Map<String, Value>,
    emails: &mut BTreeSet<String>,
) -> anyhow::Result<()> {
    if let Some(acl) = share.get("acl") {
        let acl = acl.as_object().context("Share `acl` must be an object")?;
        collect_email_array(acl.get("sharedWithEmails"), emails, "acl.sharedWithEmails")?;
        collect_email_array(
            acl.get("shared_with_emails"),
            emails,
            "acl.shared_with_emails",
        )?;
    }
    for field in ["sharedWithEmails", "shared_with_emails"] {
        collect_email_array(share.get(field), emails, field)?;
    }
    for field in [
        "accessByApp",
        "access_by_app",
        "appSettings",
        "app_settings",
    ] {
        let Some(value) = share.get(field) else {
            continue;
        };
        let by_app = value
            .as_object()
            .with_context(|| format!("Share `{field}` must be an object"))?;
        for (app, settings) in by_app {
            let settings = settings
                .as_object()
                .with_context(|| format!("Share `{field}.{app}` must be an object"))?;
            for email_field in ["sharedWithEmails", "shared_with_emails"] {
                collect_email_array(
                    settings.get(email_field),
                    emails,
                    &format!("{field}.{app}.{email_field}"),
                )?;
            }
        }
    }
    Ok(())
}

fn collect_email_array(
    value: Option<&Value>,
    emails: &mut BTreeSet<String>,
    field: &str,
) -> anyhow::Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    let values = value
        .as_array()
        .with_context(|| format!("Share `{field}` must be an array"))?;
    for value in values {
        let email = value
            .as_str()
            .with_context(|| format!("Share `{field}` entries must be strings"))?;
        let email = normalize_legacy_email(email);
        if is_plausible_email(&email) {
            emails.insert(email);
        }
    }
    Ok(())
}

fn normalize_legacy_email(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn is_plausible_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && value.len() <= 254
}

fn legacy_user_policy(share: &Map<String, Value>) -> Value {
    let mut policy = Map::new();
    if let Some(value) = share.get("parallelLimit").and_then(Value::as_u64) {
        if value > 0 {
            policy.insert("parallelLimit".to_string(), json!(value));
        }
    }
    if let Some(value) = share.get("tokenLimit").and_then(Value::as_u64) {
        if value > 0 {
            policy.insert("tokenLimit".to_string(), json!(value));
        }
    }
    if let Some(value) = share.get("expiresAt").and_then(Value::as_i64) {
        if value > 0 {
            policy.insert("expiresAt".to_string(), json!(value));
        }
    }
    policy.insert("tokenPeriod".to_string(), json!("lifetime"));
    Value::Object(policy)
}

fn read_retirement_audit(path: &Path) -> anyhow::Result<Option<DataRetirementAudit>> {
    let content = match fs::read(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let audit: DataRetirementAudit = serde_json::from_slice(&content)
        .with_context(|| format!("parse data-retirement audit {}", path.display()))?;
    ensure!(
        audit.format == AUDIT_FORMAT
            && audit.version == AUDIT_VERSION
            && audit.component == RETIRED_COMPONENT,
        "data-retirement audit has an unexpected format"
    );
    Ok(Some(audit))
}

fn count_retired_archive_payload(config_dir: &Path) -> anyhow::Result<usize> {
    let root = archive_root(config_dir);
    let metadata = match fs::symlink_metadata(&root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error).with_context(|| format!("inspect {}", root.display())),
    };
    ensure!(
        !metadata.file_type().is_symlink(),
        "retired archive root cannot be a symlink"
    );
    if metadata.is_file() {
        return Ok(1);
    }
    ensure!(metadata.is_dir(), "retired archive root has invalid type");
    count_archive_files_without_following_symlinks(&root)
}

fn remove_retired_archive_payload(config_dir: &Path) -> anyhow::Result<usize> {
    let root = archive_root(config_dir);
    let metadata = match fs::symlink_metadata(&root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error).with_context(|| format!("inspect {}", root.display())),
    };
    ensure!(
        !metadata.file_type().is_symlink(),
        "retired archive root cannot be a symlink"
    );
    let file_count = if metadata.is_file() {
        1
    } else {
        ensure!(metadata.is_dir(), "retired archive root has invalid type");
        count_archive_files_without_following_symlinks(&root)?
    };
    if metadata.is_file() {
        fs::remove_file(&root).with_context(|| format!("remove {}", root.display()))?;
        crate::infra::storage::sync_directory(config_dir)?;
        return Ok(file_count);
    }
    fs::remove_dir_all(&root).with_context(|| format!("remove {}", root.display()))?;
    crate::infra::storage::sync_directory(config_dir)?;
    Ok(file_count)
}

fn count_archive_files_without_following_symlinks(path: &Path) -> anyhow::Result<usize> {
    let mut count = 0;
    for entry in fs::read_dir(path).with_context(|| format!("read {}", path.display()))? {
        let entry = entry.with_context(|| format!("read entry in {}", path.display()))?;
        let metadata = fs::symlink_metadata(entry.path())
            .with_context(|| format!("inspect {}", entry.path().display()))?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "retired archive contains a symlink: {}",
            entry.path().display()
        );
        if metadata.is_dir() {
            count += count_archive_files_without_following_symlinks(&entry.path())?;
        } else if metadata.is_file() {
            count += 1;
        } else {
            anyhow::bail!(
                "retired archive contains an unsupported entry: {}",
                entry.path().display()
            );
        }
    }
    Ok(count)
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "cc-switch-retired-capacity-binding-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn legacy_store() -> Value {
        json!({
            "shares": [{
                "id": "share-1",
                "app": "codex",
                "providerId": "provider-1",
                "providerType": "codex",
                "acl": {
                    "sharedWithEmails": ["buyer@example.com"],
                    "publicMarketEmail": "retired@example.com",
                    "marketAccessMode": "selected"
                },
                "futureField": {"preserved": true}
            }]
        })
    }

    #[test]
    fn migration_converts_shareto_and_scrubs_legacy_fields_without_a_pii_snapshot() {
        let dir = temp_dir("scrub");
        fs::create_dir_all(&dir).unwrap();
        let source = serde_json::to_vec_pretty(&legacy_store()).unwrap();
        fs::write(shares_path(&dir), &source).unwrap();

        let loaded = load_and_migrate(&dir).unwrap();

        assert_eq!(loaded.store.shares.len(), 1);
        let migration = loaded.migration.unwrap();
        assert_eq!(migration.affected_fields, 2);
        let expected_source_sha256 = sha256_hex(&source);
        assert_eq!(
            migration.source_sha256.as_deref(),
            Some(expected_source_sha256.as_str())
        );
        assert_eq!(migration.retired_archive_files, 0);
        assert!(!archive_root(&dir).exists());
        let cleaned = fs::read_to_string(shares_path(&dir)).unwrap();
        assert!(!cleaned.contains("publicMarketEmail"));
        assert!(!cleaned.contains("retired@example.com"));
        assert!(!cleaned.contains("\"acl\""));
        assert!(cleaned.contains("buyer@example.com"));
        assert!(cleaned.contains("userGrants"));
        assert!(cleaned.contains("futureField"));
        let audit = fs::read_to_string(migration.audit_path).unwrap();
        assert!(!audit.contains("retired@example.com"));
        assert!(!audit.contains("buyer@example.com"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_scrubs_snake_case_retired_fields_from_persisted_json() {
        let mut value = json!({
            "shares": [{
                "id": "share-snake",
                "app": "codex",
                "providerId": "provider-1",
                "providerType": "codex",
                "for_sale": "Free",
                "shared_with_emails": ["snake@example.com"],
                "access_by_app": {
                    "codex": {"shared_with_emails": ["nested@example.com"]}
                },
                "app_settings": {
                    "codex": {"shared_with_emails": ["settings@example.com"]}
                },
                "official_price_percent": 25,
                "market_access_mode": "all"
            }]
        });

        let changed = migrate_legacy_share_contract(&mut value).unwrap();
        assert!(changed >= 4);
        let encoded = serde_json::to_string(&value).unwrap();
        for field in [
            "for_sale",
            "shared_with_emails",
            "access_by_app",
            "app_settings",
            "official_price_percent",
            "market_access_mode",
        ] {
            assert!(!encoded.contains(field), "retired field remains: {field}");
        }
        assert!(encoded.contains("\"freeAccess\":true"));
        assert!(encoded.contains("snake@example.com"));
        assert!(encoded.contains("nested@example.com"));
        assert!(encoded.contains("settings@example.com"));
    }

    #[test]
    fn migration_scrubs_retired_identity_fields_from_nested_snapshots() {
        let mut value = json!({
            "shares": [{
                "id": "share-nested",
                "app": "codex",
                "providerId": "provider-1",
                "providerType": "codex",
                "publicMarketEmail": "market@example.com",
                "market_subdomain": "legacy-share",
                "runtimeSnapshot": {
                    "forSale": "Yes",
                    "saleMarketKind": "token",
                    "nested": [{"marketEmail": "nested@example.com"}],
                    "keep": true
                }
            }]
        });

        let changed = migrate_legacy_share_contract(&mut value).unwrap();
        assert!(changed >= 5);
        let encoded = serde_json::to_string(&value).unwrap();
        for field in [
            "publicMarketEmail",
            "market_subdomain",
            "forSale",
            "saleMarketKind",
            "marketEmail",
        ] {
            assert!(
                !encoded.contains(field),
                "retired identity field remains: {field}"
            );
        }
        assert!(encoded.contains("\"keep\":true"));
    }

    #[test]
    fn migration_removes_a_preexisting_archive_and_is_idempotent() {
        let dir = temp_dir("archive");
        let archive = archive_root(&dir).join("shares").join("old");
        fs::create_dir_all(&archive).unwrap();
        fs::write(archive.join("shares.json"), b"private payload").unwrap();
        fs::write(archive.join("manifest.json"), b"private manifest").unwrap();
        fs::write(
            shares_path(&dir),
            serde_json::to_vec_pretty(&json!({"shares": []})).unwrap(),
        )
        .unwrap();

        let first = load_and_migrate(&dir).unwrap().migration.unwrap();
        assert_eq!(first.retired_archive_files, 2);
        assert!(!archive_root(&dir).exists());
        assert!(load_and_migrate(&dir).unwrap().migration.is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn malformed_share_store_is_not_rewritten() {
        let dir = temp_dir("malformed");
        fs::create_dir_all(&dir).unwrap();
        let source = br#"{"shares":"invalid"}"#;
        fs::write(shares_path(&dir), source).unwrap();

        assert!(load_and_migrate(&dir).is_err());
        assert_eq!(fs::read(shares_path(&dir)).unwrap(), source);
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn archive_symlink_is_rejected_without_following_it() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("symlink");
        let outside = temp_dir("outside");
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep"), b"keep").unwrap();
        symlink(&outside, archive_root(&dir)).unwrap();

        let error = load_and_migrate(&dir).unwrap_err();
        assert!(format!("{error:#}").contains("cannot be a symlink"));
        assert!(outside.join("keep").exists());
        fs::remove_file(archive_root(&dir)).unwrap();
        fs::remove_dir_all(dir).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
}
