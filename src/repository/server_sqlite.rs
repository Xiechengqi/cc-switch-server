//! Transactional repository and migration boundary for the Server's core
//! Provider, Account, Share, and Usage stores.
//!
//! Migration starts with a verified SQLite shadow, advances through a durable
//! prepared marker, and finally makes SQLite authoritative. Account and
//! Provider payloads are always copied from their encrypted persistence form;
//! plaintext credentials are never serialized into the database.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{ensure, Context};
use rusqlite::{params, Connection, OpenFlags, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::domain::accounts::store::AccountStore;
use crate::domain::providers::runtime::managed_account_binding_with_generation;
use crate::domain::providers::store::{providers_path, ProviderStore};
use crate::domain::sharing::shares::{shares_path, ShareStore};
use crate::domain::usage::store::{usage_directory, UsageLog, UsageStore};

pub(crate) const DATABASE_FILE_NAME: &str = "server-store.sqlite3";
pub(crate) const MARKER_FILE_NAME: &str = "server-store-migration.json";
const SCHEMA_VERSION: i64 = 1;
const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AuthorityState {
    ShadowVerified,
    Prepared,
    Committed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MigrationMarker {
    pub schema_version: u32,
    pub authority: AuthorityState,
    pub source_digest: String,
    pub database_file: String,
    pub prepared_at_ms: Option<i64>,
    pub committed_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_backup_dir: Option<String>,
}

pub(crate) struct AuthoritativeStores {
    pub providers: ProviderStore,
    pub accounts: AccountStore,
    pub shares: ShareStore,
    pub usage: UsageStore,
}

struct CommitObservation {
    domain: &'static str,
    started: Instant,
    succeeded: bool,
}

impl CommitObservation {
    fn start(domain: &'static str) -> Self {
        Self {
            domain,
            started: Instant::now(),
            succeeded: false,
        }
    }

    fn succeed(&mut self) {
        self.succeeded = true;
    }
}

impl Drop for CommitObservation {
    fn drop(&mut self) {
        crate::metrics::record_server_sqlite_commit(
            self.domain,
            if self.succeeded { "success" } else { "failure" },
            self.started.elapsed(),
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShadowImportReport {
    pub source_digest: String,
    pub provider_count: usize,
    pub account_count: usize,
    pub share_count: usize,
    pub usage_count: usize,
    pub source_file_count: usize,
    pub credentials_verified: usize,
}

pub(crate) struct ShadowImportInput<'a> {
    pub providers: &'a ProviderStore,
    pub accounts: &'a AccountStore,
    pub shares: &'a ShareStore,
    pub usage: &'a UsageStore,
}

#[derive(Debug, Clone)]
struct SourceFile {
    relative_path: String,
    bytes: Vec<u8>,
    sha256: String,
}

pub(crate) fn database_path(config_dir: &Path) -> PathBuf {
    config_dir.join(DATABASE_FILE_NAME)
}

pub(crate) fn marker_path(config_dir: &Path) -> PathBuf {
    config_dir.join(MARKER_FILE_NAME)
}

/// Rebuild and atomically replace the shadow database. The caller must already
/// own the data-directory lock; Server startup satisfies that precondition.
pub(crate) fn refresh_shadow(
    config_dir: &Path,
    input: ShadowImportInput<'_>,
) -> anyhow::Result<ShadowImportReport> {
    refresh_shadow_with_hook(config_dir, input, |_| Ok(()))
}

/// Fast startup path: reuse an unchanged verified shadow, otherwise rebuild it
/// from the still-authoritative legacy stores.
pub(crate) fn ensure_shadow(
    config_dir: &Path,
    input: ShadowImportInput<'_>,
) -> anyhow::Result<ShadowImportReport> {
    if read_marker(config_dir)?.is_some() {
        match verify_shadow(
            config_dir,
            ShadowImportInput {
                providers: input.providers,
                accounts: input.accounts,
                shares: input.shares,
                usage: input.usage,
            },
        ) {
            Ok(report) => return Ok(report),
            Err(error) => tracing::warn!(
                error = %error,
                "existing SQLite shadow is stale or invalid; rebuilding from authoritative legacy stores"
            ),
        }
    }
    refresh_shadow(config_dir, input)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShadowImportStage {
    SourcesRead,
    DatabaseCreated,
    TransactionCommitted,
    BeforeReplace,
    DatabaseReplaced,
    MarkerWritten,
}

pub(crate) fn refresh_shadow_with_hook(
    config_dir: &Path,
    input: ShadowImportInput<'_>,
    mut hook: impl FnMut(ShadowImportStage) -> anyhow::Result<()>,
) -> anyhow::Result<ShadowImportReport> {
    fs::create_dir_all(config_dir)
        .with_context(|| format!("create config directory {}", config_dir.display()))?;
    if read_marker(config_dir)?.is_some_and(|marker| marker.authority == AuthorityState::Committed)
    {
        anyhow::bail!("refusing to rebuild a committed SQLite authority from legacy files");
    }

    let sources = collect_sources(config_dir)?;
    hook(ShadowImportStage::SourcesRead)?;
    let source_digest = aggregate_source_digest(&sources);
    let temporary = temporary_database_path(config_dir);
    let result: anyhow::Result<ShadowImportReport> = (|| {
        let mut connection = open_database(&temporary, false)?;
        initialize_schema(&connection)?;
        hook(ShadowImportStage::DatabaseCreated)?;
        let transaction = connection
            .transaction()
            .context("begin Server SQLite shadow import")?;
        import_sources(&transaction, &sources)?;
        let credentials_verified = import_domain_rows(&transaction, config_dir, &input)?;
        set_meta(&transaction, "source_digest", &source_digest)?;
        set_meta(&transaction, "authority", "shadow_verified")?;
        set_meta(&transaction, "imported_at_ms", &now_ms().to_string())?;
        transaction
            .commit()
            .context("commit Server SQLite shadow import")?;
        hook(ShadowImportStage::TransactionCommitted)?;
        verify_connection(&connection, &input, &source_digest)?;
        connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .context("checkpoint Server SQLite shadow database")?;
        drop(connection);
        sync_file(&temporary)?;
        set_private_file_permissions(&temporary)?;
        hook(ShadowImportStage::BeforeReplace)?;
        fs::rename(&temporary, database_path(config_dir)).with_context(|| {
            format!(
                "replace Server SQLite shadow database {}",
                database_path(config_dir).display()
            )
        })?;
        crate::infra::storage::sync_directory(config_dir)?;
        hook(ShadowImportStage::DatabaseReplaced)?;
        write_marker(
            config_dir,
            &MigrationMarker {
                schema_version: SCHEMA_VERSION as u32,
                authority: AuthorityState::ShadowVerified,
                source_digest: source_digest.clone(),
                database_file: DATABASE_FILE_NAME.to_string(),
                prepared_at_ms: None,
                committed_at_ms: None,
                legacy_backup_dir: None,
            },
        )?;
        hook(ShadowImportStage::MarkerWritten)?;
        Ok(ShadowImportReport {
            source_digest,
            provider_count: input.providers.providers.len(),
            account_count: input.accounts.accounts.len(),
            share_count: input.shares.shares.len(),
            usage_count: input.usage.logs.len(),
            source_file_count: sources.len(),
            credentials_verified,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
        let _ = fs::remove_file(temporary.with_extension("sqlite3-wal"));
        let _ = fs::remove_file(temporary.with_extension("sqlite3-shm"));
    }
    result
}

pub(crate) fn verify_shadow(
    config_dir: &Path,
    input: ShadowImportInput<'_>,
) -> anyhow::Result<ShadowImportReport> {
    let marker = read_marker(config_dir)?.context("Server SQLite migration marker is missing")?;
    ensure!(
        marker.authority != AuthorityState::Committed,
        "committed SQLite authority requires the authoritative repository reader"
    );
    let sources = collect_sources(config_dir)?;
    let source_digest = aggregate_source_digest(&sources);
    ensure!(
        marker.source_digest == source_digest,
        "legacy stores changed after the SQLite shadow was built"
    );
    let connection = open_database(&database_path(config_dir), true)?;
    verify_connection(&connection, &input, &source_digest)?;
    Ok(ShadowImportReport {
        source_digest,
        provider_count: input.providers.providers.len(),
        account_count: input.accounts.accounts.len(),
        share_count: input.shares.shares.len(),
        usage_count: input.usage.logs.len(),
        source_file_count: sources.len(),
        credentials_verified: input.accounts.accounts.len(),
    })
}

pub(crate) fn read_marker(config_dir: &Path) -> anyhow::Result<Option<MigrationMarker>> {
    let path = marker_path(config_dir);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let marker: MigrationMarker =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
    ensure!(
        marker.schema_version == SCHEMA_VERSION as u32,
        "unsupported Server SQLite marker schema {}",
        marker.schema_version
    );
    ensure!(
        marker.database_file == DATABASE_FILE_NAME,
        "Server SQLite marker points outside its fixed database file"
    );
    Ok(Some(marker))
}

pub(crate) fn validate_backup_pair(stage_dir: &Path) -> anyhow::Result<()> {
    let marker =
        read_marker(stage_dir)?.context("SQLite backup is missing its migration marker")?;
    let database = database_path(stage_dir);
    ensure!(
        database.is_file(),
        "SQLite backup is missing {DATABASE_FILE_NAME}"
    );
    validate_wal_if_present(&database)?;
    let connection = open_database(&database, true)?;
    let integrity: String = connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    ensure!(
        integrity == "ok",
        "staged SQLite integrity_check failed: {integrity}"
    );
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    ensure!(
        version == SCHEMA_VERSION,
        "staged SQLite schema version is unsupported"
    );
    let source_digest: String = connection.query_row(
        "SELECT value FROM meta WHERE key='source_digest'",
        [],
        |row| row.get(0),
    )?;
    ensure!(
        source_digest == marker.source_digest,
        "SQLite marker/source digest mismatch"
    );
    let authority: String =
        connection.query_row("SELECT value FROM meta WHERE key='authority'", [], |row| {
            row.get(0)
        })?;
    ensure!(
        authority
            == match marker.authority {
                AuthorityState::ShadowVerified => "shadow_verified",
                AuthorityState::Prepared => "prepared",
                AuthorityState::Committed => "committed",
            },
        "SQLite marker/database authority mismatch"
    );
    validate_payload_digests(&connection)?;
    let fk_error: Option<(String, i64)> = connection
        .query_row("PRAGMA foreign_key_check", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()?;
    ensure!(
        fk_error.is_none(),
        "staged SQLite foreign-key graph is invalid"
    );
    let mut statement = connection.prepare(
        "SELECT relative_path,sha256,byte_length FROM legacy_blobs ORDER BY relative_path",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    for row in rows {
        let (relative, expected_sha, expected_len) = row?;
        let payload: Vec<u8> = connection.query_row(
            "SELECT payload FROM legacy_blobs WHERE relative_path=?1",
            params![relative],
            |row| row.get(0),
        )?;
        ensure!(
            i64::try_from(payload.len())? == expected_len && sha256_hex(&payload) == expected_sha,
            "staged SQLite legacy blob is corrupt: {relative}"
        );
        if marker.authority != AuthorityState::Committed {
            let path = stage_dir.join(safe_relative_path(&relative)?);
            let bytes = fs::read(&path)
                .with_context(|| format!("read staged SQLite source {}", path.display()))?;
            ensure!(
                bytes == payload,
                "staged SQLite source does not match legacy blob: {relative}"
            );
        }
    }
    drop(statement);
    validate_authoritative_graph(&connection, stage_dir)?;
    Ok(())
}

/// Rebuild the three reference-bearing stores from the encrypted rollback
/// payloads and validate their semantic graph. Materialization happens outside
/// the staged backup so validation is read-only with respect to restore input.
fn validate_authoritative_graph(connection: &Connection, key_dir: &Path) -> anyhow::Result<()> {
    let suffix: [u8; 8] = rand::random();
    let temporary = std::env::temp_dir().join(format!(
        "cc-switch-server-store-validate-{}-{}",
        std::process::id(),
        hex::encode(suffix)
    ));
    fs::create_dir(&temporary)
        .with_context(|| format!("create SQLite graph validation dir {}", temporary.display()))?;
    set_private_directory_permissions(&temporary)?;
    let result = (|| {
        materialize_blob_if_present(connection, "providers.json", &temporary)?;
        materialize_blob_if_present(connection, "accounts.json", &temporary)?;
        materialize_blob_if_present(connection, "shares.json", &temporary)?;
        let key_source = crate::domain::accounts::store::accounts_key_path(key_dir);
        if key_source.is_file() {
            let key = fs::read(&key_source)
                .with_context(|| format!("read SQLite graph key {}", key_source.display()))?;
            crate::infra::storage::write_bytes_atomic(
                &crate::domain::accounts::store::accounts_key_path(&temporary),
                &key,
            )?;
        }

        let providers = ProviderStore::load_runtime_or_default(&temporary)
            .context("load Providers while validating SQLite graph")?;
        let accounts = AccountStore::load_from_path(
            key_dir,
            &crate::domain::accounts::store::accounts_path(&temporary),
        )
        .context("load Accounts while validating SQLite graph")?;
        let shares = ShareStore::load_or_default(&temporary)
            .context("load Shares while validating SQLite graph")?;

        for (table, expected) in [
            ("providers", providers.providers.len()),
            ("accounts", accounts.accounts.len()),
            ("shares", shares.shares.len()),
        ] {
            let count: i64 =
                connection.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })?;
            ensure!(
                count == i64::try_from(expected)?,
                "staged SQLite {table} rollback payload count mismatch"
            );
        }
        ensure!(
            provider_key_hash(connection)? == expected_provider_key_hash(&providers),
            "staged SQLite Provider rollback payload does not match normalized rows"
        );
        ensure!(
            account_key_hash(connection)? == expected_account_key_hash(&accounts),
            "staged SQLite Account rollback payload does not match normalized rows"
        );
        if let Some(error) =
            crate::domain::sharing::subscription_identity::subscription_reference_graph_errors(
                &providers, &accounts, &shares,
            )
            .into_iter()
            .next()
        {
            anyhow::bail!("staged SQLite subscription reference graph is invalid: {error}");
        }
        Ok(())
    })();
    let cleanup = fs::remove_dir_all(&temporary);
    if result.is_ok() {
        cleanup.with_context(|| {
            format!("remove SQLite graph validation dir {}", temporary.display())
        })?;
    } else {
        let _ = cleanup;
    }
    result
}

/// Promote a verified shadow to the sole authority. Every step is idempotent:
/// startup can call this again after a process exit at any marker boundary.
pub(crate) fn activate_authority(config_dir: &Path) -> anyhow::Result<MigrationMarker> {
    activate_authority_with_hook(config_dir, |_| Ok(()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthorityTransitionStage {
    LegacyBackupCreated,
    DatabasePrepared,
    MarkerPrepared,
    DatabaseCommitted,
    MarkerCommitted,
    LegacyRetired,
}

pub(crate) fn activate_authority_with_hook(
    config_dir: &Path,
    mut hook: impl FnMut(AuthorityTransitionStage) -> anyhow::Result<()>,
) -> anyhow::Result<MigrationMarker> {
    let mut marker = read_marker(config_dir)?.context("SQLite migration marker is missing")?;
    let database = database_path(config_dir);
    ensure!(database.is_file(), "SQLite authority database is missing");

    let backup_relative = marker.legacy_backup_dir.clone().unwrap_or_else(|| {
        let digest_prefix = marker
            .source_digest
            .get(..16)
            .unwrap_or(&marker.source_digest);
        let imported_at_ms = database_meta_value(config_dir, "imported_at_ms")
            .ok()
            .flatten()
            .unwrap_or_else(|| "unknown-time".to_string());
        format!("migration-backups/server-store-{imported_at_ms}-{digest_prefix}")
    });
    let backup_relative_path = safe_relative_path(&backup_relative)?;
    let backup_dir = config_dir.join(&backup_relative_path);

    if marker.authority == AuthorityState::ShadowVerified {
        create_readonly_legacy_backup(config_dir, &backup_dir)?;
        hook(AuthorityTransitionStage::LegacyBackupCreated)?;
        let prepared_at_ms = now_ms();
        set_database_authority(config_dir, AuthorityState::Prepared)?;
        hook(AuthorityTransitionStage::DatabasePrepared)?;
        marker.authority = AuthorityState::Prepared;
        marker.prepared_at_ms = Some(prepared_at_ms);
        marker.legacy_backup_dir = Some(relative_path_string(&backup_relative_path)?);
        write_marker(config_dir, &marker)?;
        hook(AuthorityTransitionStage::MarkerPrepared)?;
    }

    if marker.authority == AuthorityState::Prepared {
        ensure!(
            backup_dir.is_dir(),
            "prepared SQLite migration backup is missing"
        );
        let committed_at_ms = now_ms();
        set_database_authority(config_dir, AuthorityState::Committed)?;
        hook(AuthorityTransitionStage::DatabaseCommitted)?;
        marker.authority = AuthorityState::Committed;
        marker.committed_at_ms = Some(committed_at_ms);
        marker.legacy_backup_dir = Some(relative_path_string(&backup_relative_path)?);
        write_marker(config_dir, &marker)?;
        hook(AuthorityTransitionStage::MarkerCommitted)?;
    }

    ensure!(
        marker.authority == AuthorityState::Committed,
        "SQLite authority was not committed"
    );
    verify_committed_database(config_dir, &marker)?;
    retire_legacy_sources(config_dir)?;
    hook(AuthorityTransitionStage::LegacyRetired)?;
    Ok(marker)
}

pub(crate) fn is_committed(config_dir: &Path) -> anyhow::Result<bool> {
    Ok(
        read_marker(config_dir)?
            .is_some_and(|marker| marker.authority == AuthorityState::Committed),
    )
}

pub(crate) fn authority_activation_requested() -> bool {
    std::env::var("CC_SWITCH_SERVER_SQLITE_AUTHORITY")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "committed"
            )
        })
}

pub(crate) fn load_authoritative(config_dir: &Path) -> anyhow::Result<AuthoritativeStores> {
    let marker = read_marker(config_dir)?.context("SQLite migration marker is missing")?;
    ensure!(
        marker.authority == AuthorityState::Committed,
        "SQLite authority is not committed"
    );
    verify_committed_database(config_dir, &marker)?;

    let connection = open_database(&database_path(config_dir), true)?;
    let temporary = temporary_materialization_dir(config_dir);
    fs::create_dir_all(&temporary)
        .with_context(|| format!("create SQLite materialization {}", temporary.display()))?;
    set_private_directory_permissions(&temporary)?;
    let result: anyhow::Result<(AuthoritativeStores, bool)> = (|| {
        materialize_blob_if_present(&connection, "providers.json", &temporary)?;
        materialize_blob_if_present(&connection, "accounts.json", &temporary)?;
        materialize_blob_if_present(&connection, "shares.json", &temporary)?;
        let key_source = crate::domain::accounts::store::accounts_key_path(config_dir);
        if key_source.is_file() {
            let key = fs::read(&key_source)
                .with_context(|| format!("read SQLite authority key {}", key_source.display()))?;
            crate::infra::storage::write_bytes_atomic(
                &crate::domain::accounts::store::accounts_key_path(&temporary),
                &key,
            )?;
        }
        let providers = ProviderStore::load_runtime_or_default(&temporary)
            .context("load Providers from committed SQLite authority")?;
        let accounts = AccountStore::load_from_path(
            config_dir,
            &crate::domain::accounts::store::accounts_path(&temporary),
        )
        .context("load Accounts from committed SQLite authority")?;
        let shares = ShareStore::load_or_default(&temporary)
            .context("load Shares from committed SQLite authority")?;
        let logs = load_usage_logs(&connection)?;
        let (usage, recovered) = UsageStore::from_authoritative_logs(logs);
        Ok((
            AuthoritativeStores {
                providers,
                accounts,
                shares,
                usage,
            },
            recovered,
        ))
    })();
    let cleanup = fs::remove_dir_all(&temporary);
    if let Err(error) = cleanup {
        tracing::warn!(error = %error, path = %temporary.display(), "failed to remove SQLite materialization directory");
    }
    let (stores, recovered) = result?;
    drop(connection);
    if recovered {
        replace_usage_records(config_dir, &stores.usage)
            .context("persist interrupted Usage recovery to SQLite authority")?;
    }
    Ok(stores)
}

pub(crate) fn persist_providers(
    config_dir: &Path,
    providers: &ProviderStore,
) -> anyhow::Result<()> {
    let mut observation = CommitObservation::start("providers");
    assert_committed(config_dir)?;
    let bytes = serialize_provider_store(config_dir, providers)?;
    let root: Value =
        serde_json::from_slice(&bytes).context("parse encrypted Provider persistence payload")?;
    let payloads = encrypted_provider_payloads(Some(&root), providers)?;
    let mut connection = open_database(&database_path(config_dir), false)?;
    let transaction = connection
        .transaction()
        .context("begin Provider SQLite commit")?;
    for stored in &providers.providers {
        let key = (stored.app.as_str().to_string(), stored.provider.id.clone());
        let payload = payloads
            .get(&key)
            .context("encrypted Provider record is missing")?;
        let payload_json = serde_json::to_string(payload)?;
        let changed = transaction.execute(
            "INSERT INTO providers(app,provider_id,provider_type,revision,credential_generation,payload_json,payload_sha256)
             VALUES(?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(app,provider_id) DO UPDATE SET provider_type=excluded.provider_type,revision=excluded.revision,
             credential_generation=excluded.credential_generation,payload_json=excluded.payload_json,payload_sha256=excluded.payload_sha256
             WHERE excluded.revision > providers.revision OR
               (excluded.revision = providers.revision AND excluded.credential_generation >= providers.credential_generation)",
            params![stored.app.as_str(), stored.provider.id, stored.provider_type.as_str(),
                to_sql_u64(stored.resource.revision)?, to_sql_u64(stored.resource.credential_generation)?,
                payload_json, sha256_hex(payload_json.as_bytes())],
        )?;
        ensure!(
            changed == 1,
            "stale Provider revision/generation SQLite commit rejected"
        );
    }
    transaction.execute("DELETE FROM provider_accounts", [])?;
    delete_missing_composite_keys(
        &transaction,
        "providers",
        "app",
        "provider_id",
        providers
            .providers
            .iter()
            .map(|stored| (stored.app.as_str(), stored.provider.id.as_str())),
    )?;
    for stored in &providers.providers {
        if let Some((provider_type, account_id, generation)) =
            managed_account_binding_with_generation(stored)
        {
            transaction.execute(
                "INSERT INTO provider_accounts(app,provider_id,provider_type,account_id,auth_identity_generation) VALUES(?1,?2,?3,?4,?5)",
                params![stored.app.as_str(), stored.provider.id, provider_type.as_str(), account_id, to_sql_u64(generation)?],
            )?;
        }
    }
    upsert_legacy_blob(&transaction, "providers.json", &bytes)?;
    bump_repository_generation(&transaction)?;
    transaction
        .commit()
        .context("commit Provider SQLite transaction")?;
    checkpoint_database(&connection)?;
    observation.succeed();
    Ok(())
}

pub(crate) fn persist_accounts(config_dir: &Path, accounts: &AccountStore) -> anyhow::Result<()> {
    persist_accounts_with_hook(config_dir, accounts, || Ok(()))
}

fn persist_accounts_with_hook(
    config_dir: &Path,
    accounts: &AccountStore,
    mut before_commit: impl FnMut() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut observation = CommitObservation::start("accounts");
    assert_committed(config_dir)?;
    let bytes = serialize_account_store(config_dir, accounts)?;
    let root: Value =
        serde_json::from_slice(&bytes).context("parse encrypted Account persistence payload")?;
    let payloads = encrypted_account_payloads(Some(&root), accounts)?;
    let mut connection = open_database(&database_path(config_dir), false)?;
    let transaction = connection
        .transaction()
        .context("begin Account SQLite commit")?;
    for account in &accounts.accounts {
        let key = (
            account.provider_type.as_str().to_string(),
            account.id.clone(),
        );
        let payload = payloads
            .get(&key)
            .context("encrypted Account record is missing")?;
        ensure_account_payload_secrets_are_encrypted(payload)?;
        let payload_json = serde_json::to_string(payload)?;
        let changed = transaction.execute(
            "INSERT INTO accounts(provider_type,account_id,auth_identity_generation,token_refresh_generation,payload_json,payload_sha256)
             VALUES(?1,?2,?3,?4,?5,?6)
             ON CONFLICT(provider_type,account_id) DO UPDATE SET auth_identity_generation=excluded.auth_identity_generation,
             token_refresh_generation=excluded.token_refresh_generation,payload_json=excluded.payload_json,payload_sha256=excluded.payload_sha256
             WHERE excluded.auth_identity_generation > accounts.auth_identity_generation OR
               (excluded.auth_identity_generation = accounts.auth_identity_generation AND
                excluded.token_refresh_generation >= accounts.token_refresh_generation)",
            params![account.provider_type.as_str(), account.id, to_sql_u64(account.auth_identity_generation)?,
                to_sql_u64(account.token_refresh_generation)?, payload_json, sha256_hex(payload_json.as_bytes())],
        )?;
        ensure!(
            changed == 1,
            "stale Account identity/token generation SQLite commit rejected"
        );
    }
    delete_missing_composite_keys(
        &transaction,
        "accounts",
        "provider_type",
        "account_id",
        accounts
            .accounts
            .iter()
            .map(|account| (account.provider_type.as_str(), account.id.as_str())),
    )?;
    upsert_legacy_blob(&transaction, "accounts.json", &bytes)?;
    bump_repository_generation(&transaction)?;
    before_commit()?;
    transaction
        .commit()
        .context("commit Account SQLite transaction")?;
    checkpoint_database(&connection)?;
    observation.succeed();
    Ok(())
}

pub(crate) fn persist_shares(config_dir: &Path, shares: &ShareStore) -> anyhow::Result<()> {
    let mut observation = CommitObservation::start("shares");
    assert_committed(config_dir)?;
    let bytes = serde_json::to_vec_pretty(shares).context("encode Share SQLite payload")?;
    let root: Value = serde_json::from_slice(&bytes)?;
    let payloads = share_payloads(Some(&root), shares)?;
    let mut connection = open_database(&database_path(config_dir), false)?;
    let transaction = connection
        .transaction()
        .context("begin Share SQLite commit")?;
    validate_share_revision_transition(&transaction, shares)?;
    transaction.execute("DELETE FROM share_bindings", [])?;
    transaction.execute("DELETE FROM shares", [])?;
    import_share_rows(&transaction, shares, &payloads)?;
    upsert_legacy_blob(&transaction, "shares.json", &bytes)?;
    bump_repository_generation(&transaction)?;
    transaction
        .commit()
        .context("commit Share SQLite transaction")?;
    checkpoint_database(&connection)?;
    observation.succeed();
    Ok(())
}

/// Atomically replace the complete Provider/Account/Share reference graph.
/// This is used by identity rebind operations whose three generations must
/// become visible at one commit point.
pub(crate) fn persist_reference_graph(
    config_dir: &Path,
    providers: &ProviderStore,
    accounts: &AccountStore,
    shares: &ShareStore,
) -> anyhow::Result<()> {
    let mut observation = CommitObservation::start("reference_graph");
    assert_committed(config_dir)?;
    let provider_bytes = serialize_provider_store(config_dir, providers)?;
    let account_bytes = serialize_account_store(config_dir, accounts)?;
    let share_bytes = serde_json::to_vec_pretty(shares).context("encode Share SQLite payload")?;
    let provider_root: Value = serde_json::from_slice(&provider_bytes)?;
    let account_root: Value = serde_json::from_slice(&account_bytes)?;
    let share_root: Value = serde_json::from_slice(&share_bytes)?;
    let provider_payloads = encrypted_provider_payloads(Some(&provider_root), providers)?;
    let account_payloads = encrypted_account_payloads(Some(&account_root), accounts)?;
    let share_payloads = share_payloads(Some(&share_root), shares)?;

    let mut connection = open_database(&database_path(config_dir), false)?;
    let transaction = connection
        .transaction()
        .context("begin reference graph SQLite commit")?;
    validate_share_revision_transition(&transaction, shares)?;
    transaction.execute("DELETE FROM share_bindings", [])?;
    transaction.execute("DELETE FROM shares", [])?;
    transaction.execute("DELETE FROM provider_accounts", [])?;

    for account in &accounts.accounts {
        let payload = account_payloads
            .get(&(
                account.provider_type.as_str().to_string(),
                account.id.clone(),
            ))
            .context("encrypted Account graph record is missing")?;
        ensure_account_payload_secrets_are_encrypted(payload)?;
        let payload_json = serde_json::to_string(payload)?;
        let changed = transaction.execute(
            "INSERT INTO accounts(provider_type,account_id,auth_identity_generation,token_refresh_generation,payload_json,payload_sha256)
             VALUES(?1,?2,?3,?4,?5,?6)
             ON CONFLICT(provider_type,account_id) DO UPDATE SET auth_identity_generation=excluded.auth_identity_generation,
             token_refresh_generation=excluded.token_refresh_generation,payload_json=excluded.payload_json,payload_sha256=excluded.payload_sha256
             WHERE excluded.auth_identity_generation > accounts.auth_identity_generation OR
               (excluded.auth_identity_generation = accounts.auth_identity_generation AND
                excluded.token_refresh_generation >= accounts.token_refresh_generation)",
            params![account.provider_type.as_str(), account.id, to_sql_u64(account.auth_identity_generation)?,
                to_sql_u64(account.token_refresh_generation)?, payload_json, sha256_hex(payload_json.as_bytes())],
        )?;
        ensure!(
            changed == 1,
            "stale Account graph generation SQLite commit rejected"
        );
    }
    for stored in &providers.providers {
        let payload = provider_payloads
            .get(&(stored.app.as_str().to_string(), stored.provider.id.clone()))
            .context("encrypted Provider graph record is missing")?;
        let payload_json = serde_json::to_string(payload)?;
        let changed = transaction.execute(
            "INSERT INTO providers(app,provider_id,provider_type,revision,credential_generation,payload_json,payload_sha256)
             VALUES(?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(app,provider_id) DO UPDATE SET provider_type=excluded.provider_type,revision=excluded.revision,
             credential_generation=excluded.credential_generation,payload_json=excluded.payload_json,payload_sha256=excluded.payload_sha256
             WHERE excluded.revision > providers.revision OR
               (excluded.revision = providers.revision AND excluded.credential_generation >= providers.credential_generation)",
            params![stored.app.as_str(), stored.provider.id, stored.provider_type.as_str(),
                to_sql_u64(stored.resource.revision)?, to_sql_u64(stored.resource.credential_generation)?,
                payload_json, sha256_hex(payload_json.as_bytes())],
        )?;
        ensure!(
            changed == 1,
            "stale Provider graph revision SQLite commit rejected"
        );
    }
    delete_missing_composite_keys(
        &transaction,
        "providers",
        "app",
        "provider_id",
        providers
            .providers
            .iter()
            .map(|stored| (stored.app.as_str(), stored.provider.id.as_str())),
    )?;
    delete_missing_composite_keys(
        &transaction,
        "accounts",
        "provider_type",
        "account_id",
        accounts
            .accounts
            .iter()
            .map(|account| (account.provider_type.as_str(), account.id.as_str())),
    )?;
    for stored in &providers.providers {
        if let Some((provider_type, account_id, generation)) =
            managed_account_binding_with_generation(stored)
        {
            transaction.execute(
                "INSERT INTO provider_accounts(app,provider_id,provider_type,account_id,auth_identity_generation) VALUES(?1,?2,?3,?4,?5)",
                params![stored.app.as_str(), stored.provider.id, provider_type.as_str(), account_id, to_sql_u64(generation)?],
            )?;
        }
    }
    import_share_rows(&transaction, shares, &share_payloads)?;
    upsert_legacy_blob(&transaction, "providers.json", &provider_bytes)?;
    upsert_legacy_blob(&transaction, "accounts.json", &account_bytes)?;
    upsert_legacy_blob(&transaction, "shares.json", &share_bytes)?;
    bump_repository_generation(&transaction)?;
    transaction
        .commit()
        .context("commit reference graph SQLite transaction")?;
    checkpoint_database(&connection)?;
    observation.succeed();
    Ok(())
}

pub(crate) fn upsert_usage_log(config_dir: &Path, log: &UsageLog) -> anyhow::Result<()> {
    let mut observation = CommitObservation::start("usage_upsert");
    assert_committed(config_dir)?;
    let mut connection = open_database(&database_path(config_dir), false)?;
    let transaction = connection
        .transaction()
        .context("begin Usage SQLite UPSERT")?;
    upsert_usage_row(&transaction, log)?;
    bump_repository_generation(&transaction)?;
    transaction.commit().context("commit Usage SQLite UPSERT")?;
    observation.succeed();
    Ok(())
}

pub(crate) fn replace_usage_records(config_dir: &Path, usage: &UsageStore) -> anyhow::Result<()> {
    let mut observation = CommitObservation::start("usage_replace");
    assert_committed(config_dir)?;
    let connection = open_database(&database_path(config_dir), false)?;
    replace_usage_records_in_connection(&connection, usage)?;
    observation.succeed();
    Ok(())
}

pub(crate) fn checkpoint_authority(config_dir: &Path) -> anyhow::Result<()> {
    assert_committed(config_dir)?;
    let connection = open_database(&database_path(config_dir), false)?;
    checkpoint_database(&connection)
}

fn verify_committed_database(config_dir: &Path, marker: &MigrationMarker) -> anyhow::Result<()> {
    let database = database_path(config_dir);
    validate_wal_if_present(&database)?;
    let connection = open_database(&database, true)?;
    let integrity: String = connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    ensure!(
        integrity == "ok",
        "committed SQLite integrity_check failed: {integrity}"
    );
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    ensure!(
        version == SCHEMA_VERSION,
        "committed SQLite schema version is unsupported"
    );
    let authority: String =
        connection.query_row("SELECT value FROM meta WHERE key='authority'", [], |row| {
            row.get(0)
        })?;
    ensure!(
        authority == "committed",
        "SQLite database authority is not committed"
    );
    let source_digest: String = connection.query_row(
        "SELECT value FROM meta WHERE key='source_digest'",
        [],
        |row| row.get(0),
    )?;
    ensure!(
        source_digest == marker.source_digest,
        "committed SQLite marker/source digest mismatch"
    );
    validate_payload_digests(&connection)?;
    let fk_error: Option<(String, i64)> = connection
        .query_row("PRAGMA foreign_key_check", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()?;
    ensure!(
        fk_error.is_none(),
        "committed SQLite foreign-key graph is invalid"
    );
    Ok(())
}

/// SQLite treats an invalid WAL tail as an incomplete crash write and may
/// silently ignore it. At an authority/restore boundary every durable frame is
/// part of the repository, so validate the complete envelope and rolling
/// checksums before allowing SQLite to recover it.
fn validate_wal_if_present(database: &Path) -> anyhow::Result<()> {
    let wal_path = database.with_file_name(format!(
        "{}-wal",
        database
            .file_name()
            .and_then(|value| value.to_str())
            .context("SQLite database filename must be UTF-8")?
    ));
    let file = match fs::File::open(&wal_path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("open SQLite WAL {}", wal_path.display()))
        }
    };
    let length = file
        .metadata()
        .with_context(|| format!("stat SQLite WAL {}", wal_path.display()))?
        .len();
    if length == 0 {
        return Ok(());
    }
    ensure!(length >= 32, "SQLite WAL is truncated before its header");
    let mut reader = BufReader::new(file);
    let mut header = [0_u8; 32];
    reader
        .read_exact(&mut header)
        .with_context(|| format!("read SQLite WAL header {}", wal_path.display()))?;
    let magic = u32::from_be_bytes(header[0..4].try_into()?);
    let checksum_little_endian = match magic {
        0x377f_0682 => true,
        0x377f_0683 => false,
        _ => anyhow::bail!("SQLite WAL has an invalid magic value"),
    };
    ensure!(
        u32::from_be_bytes(header[4..8].try_into()?) == 3_007_000,
        "SQLite WAL format version is unsupported"
    );
    let encoded_page_size = u32::from_be_bytes(header[8..12].try_into()?);
    let page_size = if encoded_page_size == 1 {
        65_536_u32
    } else {
        encoded_page_size
    };
    ensure!(
        (512..=65_536).contains(&page_size) && page_size.is_power_of_two(),
        "SQLite WAL page size is invalid"
    );
    let frame_size = 24_u64.saturating_add(u64::from(page_size));
    ensure!(
        (length - 32) % frame_size == 0,
        "SQLite WAL ends with a partial frame"
    );

    let mut checksum = wal_checksum(checksum_little_endian, [0, 0], &header[..24])?;
    let expected_header = [
        u32::from_be_bytes(header[24..28].try_into()?),
        u32::from_be_bytes(header[28..32].try_into()?),
    ];
    ensure!(
        checksum == expected_header,
        "SQLite WAL header checksum mismatch"
    );
    let salt = &header[16..24];
    let mut frame = vec![0_u8; usize::try_from(frame_size)?];
    let frame_count = (length - 32) / frame_size;
    for index in 0..frame_count {
        reader
            .read_exact(&mut frame)
            .with_context(|| format!("read SQLite WAL frame {}", index + 1))?;
        ensure!(
            frame[8..16] == salt[..],
            "SQLite WAL frame salt mismatch at frame {}",
            index + 1
        );
        checksum = wal_checksum(checksum_little_endian, checksum, &frame[..8])?;
        checksum = wal_checksum(checksum_little_endian, checksum, &frame[24..])?;
        let expected = [
            u32::from_be_bytes(frame[16..20].try_into()?),
            u32::from_be_bytes(frame[20..24].try_into()?),
        ];
        ensure!(
            checksum == expected,
            "SQLite WAL checksum mismatch at frame {}",
            index + 1
        );
    }
    Ok(())
}

fn wal_checksum(
    little_endian: bool,
    mut state: [u32; 2],
    bytes: &[u8],
) -> anyhow::Result<[u32; 2]> {
    ensure!(
        bytes.len().is_multiple_of(8),
        "SQLite WAL checksum input is misaligned"
    );
    for pair in bytes.chunks_exact(8) {
        let first = if little_endian {
            u32::from_le_bytes(pair[..4].try_into()?)
        } else {
            u32::from_be_bytes(pair[..4].try_into()?)
        };
        let second = if little_endian {
            u32::from_le_bytes(pair[4..].try_into()?)
        } else {
            u32::from_be_bytes(pair[4..].try_into()?)
        };
        state[0] = state[0].wrapping_add(first).wrapping_add(state[1]);
        state[1] = state[1].wrapping_add(second).wrapping_add(state[0]);
    }
    Ok(state)
}

fn validate_payload_digests(connection: &Connection) -> anyhow::Result<()> {
    for table in ["providers", "shares", "usage_records"] {
        let mut statement =
            connection.prepare(&format!("SELECT payload_json,payload_sha256 FROM {table}"))?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (payload, expected_sha) = row?;
            ensure!(
                sha256_hex(payload.as_bytes()) == expected_sha,
                "SQLite {table} payload digest mismatch"
            );
        }
    }
    let mut statement = connection.prepare("SELECT payload_json,payload_sha256 FROM accounts")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (payload, expected_sha) = row?;
        ensure!(
            sha256_hex(payload.as_bytes()) == expected_sha,
            "SQLite accounts payload digest mismatch"
        );
        let value: Value =
            serde_json::from_str(&payload).context("decode encrypted SQLite Account payload")?;
        ensure_account_payload_secrets_are_encrypted(&value)?;
    }
    Ok(())
}

fn assert_committed(config_dir: &Path) -> anyhow::Result<MigrationMarker> {
    let marker = read_marker(config_dir)?.context("SQLite migration marker is missing")?;
    ensure!(
        marker.authority == AuthorityState::Committed,
        "SQLite authority is not committed"
    );
    let connection = open_database(&database_path(config_dir), true)?;
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    ensure!(
        version == SCHEMA_VERSION,
        "SQLite writer schema version is unsupported"
    );
    let authority: String =
        connection.query_row("SELECT value FROM meta WHERE key='authority'", [], |row| {
            row.get(0)
        })?;
    ensure!(
        authority == "committed",
        "SQLite writer authority is not committed"
    );
    Ok(marker)
}

fn authority_label(authority: AuthorityState) -> &'static str {
    match authority {
        AuthorityState::ShadowVerified => "shadow_verified",
        AuthorityState::Prepared => "prepared",
        AuthorityState::Committed => "committed",
    }
}

fn database_meta_value(config_dir: &Path, key: &str) -> anyhow::Result<Option<String>> {
    let connection = open_database(&database_path(config_dir), true)?;
    connection
        .query_row("SELECT value FROM meta WHERE key=?1", params![key], |row| {
            row.get(0)
        })
        .optional()
        .context("read Server SQLite metadata")
}

fn set_database_authority(config_dir: &Path, authority: AuthorityState) -> anyhow::Result<()> {
    let mut connection = open_database(&database_path(config_dir), false)?;
    let transaction = connection
        .transaction()
        .context("begin SQLite authority transition")?;
    transaction.execute(
        "INSERT INTO meta(key,value) VALUES('authority',?1)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![authority_label(authority)],
    )?;
    bump_repository_generation(&transaction)?;
    transaction
        .commit()
        .context("commit SQLite authority transition")?;
    checkpoint_database(&connection)
}

fn create_readonly_legacy_backup(config_dir: &Path, backup_dir: &Path) -> anyhow::Result<()> {
    const COMPLETE_FILE: &str = ".server-store-backup-complete";
    if backup_dir.exists() {
        ensure!(
            backup_dir.is_dir(),
            "legacy migration backup path is not a directory"
        );
        if backup_dir.join(COMPLETE_FILE).is_file() {
            return Ok(());
        }
        fs::remove_dir_all(backup_dir).with_context(|| {
            format!(
                "remove incomplete legacy migration backup {}",
                backup_dir.display()
            )
        })?;
    }
    fs::create_dir_all(backup_dir)
        .with_context(|| format!("create legacy migration backup {}", backup_dir.display()))?;
    set_private_directory_permissions(backup_dir)?;
    for source in core_legacy_sources(config_dir) {
        if !source.exists() {
            continue;
        }
        let relative = source
            .strip_prefix(config_dir)
            .context("legacy source escaped config directory")?;
        copy_path_recursively(&source, &backup_dir.join(relative))?;
    }
    crate::infra::storage::write_bytes_atomic(
        &backup_dir.join(COMPLETE_FILE),
        b"cc-switch-server-store-backup-v1\n",
    )?;
    make_readonly_recursively(backup_dir)?;
    crate::infra::storage::sync_directory(backup_dir)?;
    if let Some(parent) = backup_dir.parent() {
        crate::infra::storage::sync_directory(parent)?;
    }
    Ok(())
}

fn retire_legacy_sources(config_dir: &Path) -> anyhow::Result<()> {
    for source in core_legacy_sources(config_dir) {
        if source.is_dir() {
            fs::remove_dir_all(&source)
                .with_context(|| format!("retire legacy directory {}", source.display()))?;
        } else if source.exists() {
            fs::remove_file(&source)
                .with_context(|| format!("retire legacy file {}", source.display()))?;
        }
    }
    crate::infra::storage::sync_directory(config_dir)
}

fn core_legacy_sources(config_dir: &Path) -> [PathBuf; 4] {
    [
        providers_path(config_dir),
        crate::domain::accounts::store::accounts_path(config_dir),
        shares_path(config_dir),
        usage_directory(config_dir),
    ]
}

fn copy_path_recursively(source: &Path, destination: &Path) -> anyhow::Result<()> {
    if source.is_dir() {
        fs::create_dir_all(destination)
            .with_context(|| format!("create backup directory {}", destination.display()))?;
        set_private_directory_permissions(destination)?;
        let mut entries = fs::read_dir(source)
            .with_context(|| format!("read legacy source {}", source.display()))?
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            ensure!(
                !entry.path().is_symlink(),
                "legacy migration source may not be a symlink"
            );
            copy_path_recursively(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, destination).with_context(|| {
            format!(
                "copy legacy migration source {} to {}",
                source.display(),
                destination.display()
            )
        })?;
        sync_file(destination)?;
    }
    Ok(())
}

fn make_readonly_recursively(path: &Path) -> anyhow::Result<()> {
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            make_readonly_recursively(&entry?.path())?;
        }
    }
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("make migration backup readonly {}", path.display()))
}

fn temporary_materialization_dir(config_dir: &Path) -> PathBuf {
    let suffix: [u8; 8] = rand::random();
    config_dir.join(format!(".server-store-materialize-{}", hex::encode(suffix)))
}

fn materialize_blob_if_present(
    connection: &Connection,
    relative: &str,
    destination_dir: &Path,
) -> anyhow::Result<()> {
    let payload = connection
        .query_row(
            "SELECT payload FROM legacy_blobs WHERE relative_path=?1",
            params![relative],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?;
    if let Some(payload) = payload {
        crate::infra::storage::write_bytes_atomic(&destination_dir.join(relative), &payload)?;
    }
    Ok(())
}

fn serialize_provider_store(
    config_dir: &Path,
    providers: &ProviderStore,
) -> anyhow::Result<Vec<u8>> {
    let temporary = temporary_materialization_dir(config_dir);
    fs::create_dir_all(&temporary)?;
    set_private_directory_permissions(&temporary)?;
    let result = (|| {
        providers.save(&temporary)?;
        fs::read(providers_path(&temporary)).context("read serialized Provider store")
    })();
    let _ = fs::remove_dir_all(&temporary);
    result
}

fn serialize_account_store(config_dir: &Path, accounts: &AccountStore) -> anyhow::Result<Vec<u8>> {
    let temporary = temporary_materialization_dir(config_dir);
    fs::create_dir_all(&temporary)?;
    set_private_directory_permissions(&temporary)?;
    let result = (|| {
        let path = crate::domain::accounts::store::accounts_path(&temporary);
        accounts.save_to_path(config_dir, &path)?;
        fs::read(path).context("read serialized Account store")
    })();
    let _ = fs::remove_dir_all(&temporary);
    result
}

fn delete_missing_composite_keys<'a>(
    transaction: &Transaction<'_>,
    table: &str,
    first_column: &str,
    second_column: &str,
    expected: impl Iterator<Item = (&'a str, &'a str)>,
) -> anyhow::Result<()> {
    let expected = expected
        .map(|(first, second)| (first.to_string(), second.to_string()))
        .collect::<std::collections::BTreeSet<_>>();
    let query = format!("SELECT {first_column},{second_column} FROM {table}");
    let mut statement = transaction.prepare(&query)?;
    let existing = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    let delete = format!("DELETE FROM {table} WHERE {first_column}=?1 AND {second_column}=?2");
    for (first, second) in existing {
        if !expected.contains(&(first.clone(), second.clone())) {
            transaction.execute(&delete, params![first, second])?;
        }
    }
    Ok(())
}

fn validate_share_revision_transition(
    transaction: &Transaction<'_>,
    incoming: &ShareStore,
) -> anyhow::Result<()> {
    let incoming_revisions = incoming
        .shares
        .iter()
        .map(|share| (share.id.as_str(), share.config_revision))
        .collect::<BTreeMap<_, _>>();
    ensure!(
        incoming_revisions.len() == incoming.shares.len(),
        "duplicate Share identity in SQLite commit"
    );
    let deletion_tombstones = incoming
        .pending_router_deletes
        .iter()
        .map(|tombstone| tombstone.share_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut statement = transaction.prepare("SELECT share_id,config_revision FROM shares")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (share_id, current_revision) = row?;
        let current_revision = u64::try_from(current_revision)
            .context("stored Share revision must be non-negative")?;
        match incoming_revisions.get(share_id.as_str()) {
            Some(incoming_revision) => ensure!(
                *incoming_revision >= current_revision,
                "stale Share config revision SQLite commit rejected"
            ),
            None => ensure!(
                deletion_tombstones.contains(share_id.as_str()),
                "stale Share snapshot attempted deletion without a tombstone"
            ),
        }
    }
    Ok(())
}

fn import_share_rows(
    transaction: &Transaction<'_>,
    shares: &ShareStore,
    payloads: &BTreeMap<String, Value>,
) -> anyhow::Result<()> {
    for share in &shares.shares {
        let payload = payloads
            .get(&share.id)
            .context("Share persistence record is missing")?;
        let payload_json = serde_json::to_string(payload)?;
        transaction.execute(
            "INSERT INTO shares(share_id,app,provider_id,provider_type,config_revision,payload_json,payload_sha256)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![share.id, share.app.as_str(), share.provider_id, share.provider_type.as_str(),
                to_sql_u64(share.config_revision)?, payload_json, sha256_hex(payload_json.as_bytes())],
        )?;
        let mut bindings = BTreeMap::new();
        bindings.insert(
            share.app.as_str(),
            (share.provider_id.as_str(), share.provider_type.as_str()),
        );
        for binding in &share.bindings {
            bindings.insert(
                binding.app.as_str(),
                (binding.provider_id.as_str(), binding.provider_type.as_str()),
            );
        }
        for (app, (provider_id, provider_type)) in bindings {
            transaction.execute(
                "INSERT INTO share_bindings(share_id,app,provider_id,provider_type) VALUES(?1,?2,?3,?4)",
                params![share.id, app, provider_id, provider_type],
            )?;
        }
    }
    Ok(())
}

fn upsert_usage_row(transaction: &Transaction<'_>, log: &UsageLog) -> anyhow::Result<()> {
    let payload_json = serde_json::to_string(log)?;
    let changed = transaction.execute(
        "INSERT INTO usage_records(request_id,created_at_ms,share_id,provider_id,usage_revision,payload_json,payload_sha256)
         VALUES(?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(request_id) DO UPDATE SET created_at_ms=excluded.created_at_ms,share_id=excluded.share_id,
         provider_id=excluded.provider_id,usage_revision=excluded.usage_revision,payload_json=excluded.payload_json,
         payload_sha256=excluded.payload_sha256
         WHERE excluded.usage_revision >= usage_records.usage_revision",
        params![log.request_id, to_sql_u128(log.created_at_ms)?, log.share_id, log.provider_id,
            to_sql_u64(log.usage_revision)?, payload_json, sha256_hex(payload_json.as_bytes())],
    )?;
    ensure!(changed == 1, "stale Usage revision SQLite commit rejected");
    Ok(())
}

fn replace_usage_records_in_connection(
    connection: &Connection,
    usage: &UsageStore,
) -> anyhow::Result<()> {
    let transaction = connection
        .unchecked_transaction()
        .context("begin Usage SQLite replace")?;
    transaction.execute("DELETE FROM usage_records", [])?;
    for log in &usage.logs {
        upsert_usage_row(&transaction, log)?;
    }
    bump_repository_generation(&transaction)?;
    transaction
        .commit()
        .context("commit Usage SQLite replace")?;
    checkpoint_database(connection)
}

fn load_usage_logs(connection: &Connection) -> anyhow::Result<Vec<UsageLog>> {
    let mut statement = connection.prepare(
        "SELECT payload_json,payload_sha256 FROM usage_records ORDER BY created_at_ms,request_id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut logs = Vec::new();
    for row in rows {
        let (payload, expected_sha) = row?;
        ensure!(
            sha256_hex(payload.as_bytes()) == expected_sha,
            "Usage SQLite payload digest mismatch"
        );
        logs.push(serde_json::from_str(&payload).context("decode Usage SQLite payload")?);
    }
    Ok(logs)
}

fn upsert_legacy_blob(
    transaction: &Transaction<'_>,
    relative: &str,
    bytes: &[u8],
) -> anyhow::Result<()> {
    transaction.execute(
        "INSERT INTO legacy_blobs(relative_path,payload,sha256,byte_length) VALUES(?1,?2,?3,?4)
         ON CONFLICT(relative_path) DO UPDATE SET payload=excluded.payload,sha256=excluded.sha256,byte_length=excluded.byte_length",
        params![relative, bytes, sha256_hex(bytes), i64::try_from(bytes.len())?],
    )?;
    Ok(())
}

fn bump_repository_generation(transaction: &Transaction<'_>) -> anyhow::Result<()> {
    transaction.execute(
        "INSERT INTO meta(key,value) VALUES('repository_generation','1')
         ON CONFLICT(key) DO UPDATE SET value=CAST(CAST(value AS INTEGER)+1 AS TEXT)",
        [],
    )?;
    Ok(())
}

fn checkpoint_database(connection: &Connection) -> anyhow::Result<()> {
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .context("checkpoint authoritative Server SQLite database")
}

pub(crate) fn write_marker(config_dir: &Path, marker: &MigrationMarker) -> anyhow::Result<()> {
    crate::infra::storage::write_json_pretty(&marker_path(config_dir), marker)
        .context("write Server SQLite migration marker")
}

/// Create a transactionally consistent standalone SQLite backup.
#[allow(dead_code)]
pub(crate) fn online_backup(config_dir: &Path, destination: &Path) -> anyhow::Result<()> {
    let source_path = database_path(config_dir);
    let source = open_database(&source_path, true)?;
    if destination.exists() {
        anyhow::bail!(
            "SQLite backup destination already exists: {}",
            destination.display()
        );
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create SQLite backup directory {}", parent.display()))?;
    }
    let mut target = open_database(destination, false)?;
    let backup = rusqlite::backup::Backup::new(&source, &mut target)
        .context("start SQLite online backup")?;
    backup
        .run_to_completion(128, Duration::from_millis(5), None)
        .context("complete SQLite online backup")?;
    drop(backup);
    drop(target);
    sync_file(destination)?;
    set_private_file_permissions(destination)?;
    Ok(())
}

/// Exact DB-to-legacy rollback export. Payloads are the original encrypted
/// source bytes, not reserialized in-memory Accounts or Providers.
#[allow(dead_code)]
pub(crate) fn export_legacy(config_dir: &Path, destination_dir: &Path) -> anyhow::Result<usize> {
    ensure!(
        !destination_dir.exists(),
        "legacy export destination already exists: {}",
        destination_dir.display()
    );
    fs::create_dir_all(destination_dir)
        .with_context(|| format!("create legacy export {}", destination_dir.display()))?;
    let connection = open_database(&database_path(config_dir), true)?;
    let committed = connection
        .query_row("SELECT value FROM meta WHERE key='authority'", [], |row| {
            row.get::<_, String>(0)
        })
        .optional()?
        .is_some_and(|authority| authority == "committed");
    let mut statement = connection
        .prepare("SELECT relative_path, payload, sha256 FROM legacy_blobs ORDER BY relative_path")
        .context("query SQLite legacy blobs")?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .context("read SQLite legacy blobs")?;
    let mut count = 0usize;
    for row in rows {
        let (relative, bytes, expected_sha) = row.context("decode SQLite legacy blob")?;
        if committed && (relative == "usage" || relative.starts_with("usage/")) {
            continue;
        }
        let relative = safe_relative_path(&relative)?;
        ensure!(
            sha256_hex(&bytes) == expected_sha,
            "SQLite legacy blob digest mismatch"
        );
        let path = destination_dir.join(relative);
        crate::infra::storage::write_bytes_atomic(&path, &bytes)?;
        count = count.saturating_add(1);
    }
    drop(statement);
    if committed {
        let (usage, _) = UsageStore::from_authoritative_logs(load_usage_logs(&connection)?);
        usage
            .save(destination_dir)
            .context("export authoritative SQLite Usage to legacy format")?;
        count = count.saturating_add(
            collect_sources(destination_dir)?
                .into_iter()
                .filter(|source| source.relative_path.starts_with("usage/"))
                .count(),
        );
    }
    // The root key deliberately remains outside SQLite so disclosure of the
    // database alone does not collapse the field-encryption boundary. An
    // offline rollback therefore also requires the still-present data-dir key.
    let key_source = crate::domain::accounts::store::accounts_key_path(config_dir);
    if key_source.is_file() {
        let key = fs::read(&key_source)
            .with_context(|| format!("read rollback key {}", key_source.display()))?;
        crate::infra::storage::write_bytes_atomic(
            &crate::domain::accounts::store::accounts_key_path(destination_dir),
            &key,
        )?;
        count = count.saturating_add(1);
    }
    Ok(count)
}

fn open_database(path: &Path, read_only: bool) -> anyhow::Result<Connection> {
    let flags = if read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
    };
    let connection = Connection::open_with_flags(path, flags)
        .with_context(|| format!("open Server SQLite database {}", path.display()))?;
    connection.busy_timeout(SQLITE_BUSY_TIMEOUT)?;
    connection.execute_batch(
        "PRAGMA foreign_keys=ON;
         PRAGMA trusted_schema=OFF;
         PRAGMA recursive_triggers=OFF;",
    )?;
    if !read_only {
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             PRAGMA wal_autocheckpoint=1000;",
        )?;
    }
    Ok(connection)
}

fn initialize_schema(connection: &Connection) -> anyhow::Result<()> {
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
         CREATE TABLE schema_migrations(
           version INTEGER PRIMARY KEY,
           name TEXT NOT NULL,
           applied_at_ms INTEGER NOT NULL,
           checksum TEXT NOT NULL
         ) STRICT;
         CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
         CREATE TABLE legacy_blobs(
           relative_path TEXT PRIMARY KEY,
           payload BLOB NOT NULL,
           sha256 TEXT NOT NULL,
           byte_length INTEGER NOT NULL CHECK(byte_length >= 0)
         ) STRICT;
         CREATE TABLE providers(
           app TEXT NOT NULL,
           provider_id TEXT NOT NULL,
           provider_type TEXT NOT NULL,
           revision INTEGER NOT NULL CHECK(revision >= 0),
           credential_generation INTEGER NOT NULL CHECK(credential_generation >= 0),
           payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
           payload_sha256 TEXT NOT NULL,
           PRIMARY KEY(app, provider_id)
         ) STRICT;
         CREATE TABLE accounts(
           provider_type TEXT NOT NULL,
           account_id TEXT NOT NULL,
           auth_identity_generation INTEGER NOT NULL CHECK(auth_identity_generation >= 0),
           token_refresh_generation INTEGER NOT NULL CHECK(token_refresh_generation >= 0),
           payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
           payload_sha256 TEXT NOT NULL,
           PRIMARY KEY(provider_type, account_id)
         ) STRICT;
         CREATE TABLE provider_accounts(
           app TEXT NOT NULL,
           provider_id TEXT NOT NULL,
           provider_type TEXT NOT NULL,
           account_id TEXT NOT NULL,
           auth_identity_generation INTEGER NOT NULL,
           PRIMARY KEY(app, provider_id),
           FOREIGN KEY(app, provider_id) REFERENCES providers(app, provider_id),
           FOREIGN KEY(provider_type, account_id) REFERENCES accounts(provider_type, account_id)
         ) STRICT;
         CREATE TABLE shares(
           share_id TEXT PRIMARY KEY,
           app TEXT NOT NULL,
           provider_id TEXT NOT NULL,
           provider_type TEXT NOT NULL,
           config_revision INTEGER NOT NULL CHECK(config_revision >= 0),
           payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
           payload_sha256 TEXT NOT NULL,
           FOREIGN KEY(app, provider_id) REFERENCES providers(app, provider_id)
         ) STRICT;
         CREATE TABLE share_bindings(
           share_id TEXT NOT NULL,
           app TEXT NOT NULL,
           provider_id TEXT NOT NULL,
           provider_type TEXT NOT NULL,
           PRIMARY KEY(share_id, app),
           FOREIGN KEY(share_id) REFERENCES shares(share_id) ON DELETE CASCADE,
           FOREIGN KEY(app, provider_id) REFERENCES providers(app, provider_id)
         ) STRICT;
         CREATE TABLE usage_records(
           request_id TEXT PRIMARY KEY,
           created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
           share_id TEXT,
           provider_id TEXT,
           usage_revision INTEGER NOT NULL CHECK(usage_revision >= 0),
           payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
           payload_sha256 TEXT NOT NULL
         ) STRICT;
         CREATE INDEX usage_records_created_idx ON usage_records(created_at_ms, request_id);
         CREATE INDEX usage_records_share_idx ON usage_records(share_id, created_at_ms);
         INSERT INTO schema_migrations(version,name,applied_at_ms,checksum)
         VALUES(1,'initial_server_store',0,'sha256:server-store-schema-v1');
         PRAGMA user_version=1;
         COMMIT;",
        )
        .context("initialize Server SQLite schema")
}

fn import_sources(transaction: &Transaction<'_>, sources: &[SourceFile]) -> anyhow::Result<()> {
    let mut statement = transaction.prepare(
        "INSERT INTO legacy_blobs(relative_path,payload,sha256,byte_length) VALUES(?1,?2,?3,?4)",
    )?;
    for source in sources {
        statement.execute(params![
            source.relative_path,
            source.bytes,
            source.sha256,
            i64::try_from(source.bytes.len()).unwrap_or(i64::MAX),
        ])?;
    }
    Ok(())
}

fn import_domain_rows(
    transaction: &Transaction<'_>,
    config_dir: &Path,
    input: &ShadowImportInput<'_>,
) -> anyhow::Result<usize> {
    let provider_root = read_json_if_exists(&providers_path(config_dir))?;
    let account_root =
        read_json_if_exists(&crate::domain::accounts::store::accounts_path(config_dir))?;
    let share_root = read_json_if_exists(&shares_path(config_dir))?;

    let provider_payloads = encrypted_provider_payloads(provider_root.as_ref(), input.providers)?;
    ensure!(
        provider_payloads.len() == input.providers.providers.len(),
        "Provider source/runtime count mismatch"
    );
    for stored in &input.providers.providers {
        let key = (stored.app.as_str().to_string(), stored.provider.id.clone());
        let payload = provider_payloads
            .get(&key)
            .context("Provider source record is missing")?;
        let payload_json = serde_json::to_string(payload)?;
        transaction.execute(
            "INSERT INTO providers(app,provider_id,provider_type,revision,credential_generation,payload_json,payload_sha256)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                stored.app.as_str(),
                stored.provider.id,
                stored.provider_type.as_str(),
                to_sql_u64(stored.resource.revision)?,
                to_sql_u64(stored.resource.credential_generation)?,
                payload_json,
                sha256_hex(payload_json.as_bytes()),
            ],
        )?;
    }

    let account_payloads = encrypted_account_payloads(account_root.as_ref(), input.accounts)?;
    ensure!(
        account_payloads.len() == input.accounts.accounts.len(),
        "Account source/runtime count mismatch"
    );
    for account in &input.accounts.accounts {
        let key = (
            account.provider_type.as_str().to_string(),
            account.id.clone(),
        );
        let payload = account_payloads
            .get(&key)
            .context("Account source record is missing")?;
        ensure_account_payload_secrets_are_encrypted(payload)?;
        let payload_json = serde_json::to_string(payload)?;
        transaction.execute(
            "INSERT INTO accounts(provider_type,account_id,auth_identity_generation,token_refresh_generation,payload_json,payload_sha256)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                account.provider_type.as_str(),
                account.id,
                to_sql_u64(account.auth_identity_generation)?,
                to_sql_u64(account.token_refresh_generation)?,
                payload_json,
                sha256_hex(payload_json.as_bytes()),
            ],
        )?;
    }

    for stored in &input.providers.providers {
        if let Some((provider_type, account_id, auth_generation)) =
            managed_account_binding_with_generation(stored)
        {
            transaction.execute(
                "INSERT INTO provider_accounts(app,provider_id,provider_type,account_id,auth_identity_generation)
                 VALUES(?1,?2,?3,?4,?5)",
                params![
                    stored.app.as_str(),
                    stored.provider.id,
                    provider_type.as_str(),
                    account_id,
                    to_sql_u64(auth_generation)?,
                ],
            )?;
        }
    }

    let share_payloads = share_payloads(share_root.as_ref(), input.shares)?;
    ensure!(
        share_payloads.len() == input.shares.shares.len(),
        "Share source/runtime count mismatch"
    );
    for share in &input.shares.shares {
        let payload = share_payloads
            .get(&share.id)
            .context("Share source record is missing")?;
        let payload_json = serde_json::to_string(payload)?;
        transaction.execute(
            "INSERT INTO shares(share_id,app,provider_id,provider_type,config_revision,payload_json,payload_sha256)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                share.id,
                share.app.as_str(),
                share.provider_id,
                share.provider_type.as_str(),
                to_sql_u64(share.config_revision)?,
                payload_json,
                sha256_hex(payload_json.as_bytes()),
            ],
        )?;
        let mut bindings = BTreeMap::new();
        bindings.insert(
            share.app.as_str().to_string(),
            (share.provider_id.as_str(), share.provider_type.as_str()),
        );
        for binding in &share.bindings {
            bindings.insert(
                binding.app.as_str().to_string(),
                (binding.provider_id.as_str(), binding.provider_type.as_str()),
            );
        }
        for (app, (provider_id, provider_type)) in bindings {
            transaction.execute(
                "INSERT INTO share_bindings(share_id,app,provider_id,provider_type) VALUES(?1,?2,?3,?4)",
                params![share.id, app, provider_id, provider_type],
            )?;
        }
    }

    for log in &input.usage.logs {
        let payload_json = serde_json::to_string(log)?;
        transaction.execute(
            "INSERT INTO usage_records(request_id,created_at_ms,share_id,provider_id,usage_revision,payload_json,payload_sha256)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                log.request_id,
                to_sql_u128(log.created_at_ms)?,
                log.share_id,
                log.provider_id,
                to_sql_u64(log.usage_revision)?,
                payload_json,
                sha256_hex(payload_json.as_bytes()),
            ],
        )?;
    }

    // Loading these stores already authenticated every credential envelope.
    // Matching every encrypted source key and generation to that runtime view
    // makes that decryption validation part of the shadow-import receipt.
    Ok(input.accounts.accounts.len())
}

fn verify_connection(
    connection: &Connection,
    input: &ShadowImportInput<'_>,
    source_digest: &str,
) -> anyhow::Result<()> {
    let integrity: String = connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    ensure!(
        integrity == "ok",
        "SQLite integrity_check failed: {integrity}"
    );
    let foreign_keys: i64 = connection.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    ensure!(foreign_keys == 1, "SQLite foreign keys are disabled");
    let stored_digest: String = connection.query_row(
        "SELECT value FROM meta WHERE key='source_digest'",
        [],
        |row| row.get(0),
    )?;
    ensure!(
        stored_digest == source_digest,
        "SQLite source digest mismatch"
    );
    for (table, expected) in [
        ("providers", input.providers.providers.len()),
        ("accounts", input.accounts.accounts.len()),
        ("shares", input.shares.shares.len()),
        ("usage_records", input.usage.logs.len()),
    ] {
        let count: i64 =
            connection.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })?;
        ensure!(
            count == i64::try_from(expected)?,
            "SQLite {table} count mismatch"
        );
    }
    let fk_error: Option<(String, i64)> = connection
        .query_row("PRAGMA foreign_key_check", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()?;
    ensure!(fk_error.is_none(), "SQLite foreign-key graph is invalid");
    ensure!(
        provider_key_hash(connection)? == expected_provider_key_hash(input.providers),
        "SQLite Provider key/revision hash mismatch"
    );
    ensure!(
        account_key_hash(connection)? == expected_account_key_hash(input.accounts),
        "SQLite Account key/generation hash mismatch"
    );
    Ok(())
}

fn encrypted_provider_payloads(
    root: Option<&Value>,
    providers: &ProviderStore,
) -> anyhow::Result<BTreeMap<(String, String), Value>> {
    if providers.providers.is_empty() {
        return Ok(BTreeMap::new());
    }
    let root = root.context("providers.json is missing")?;
    ensure!(
        root.get("format").and_then(Value::as_str) == Some("cc-switch-provider-store"),
        "non-empty legacy Provider S1 store must be migrated to encrypted S2 before SQLite shadow import"
    );
    let records = root
        .get("records")
        .and_then(Value::as_object)
        .context("Provider S2 records are missing")?;
    let mut output = BTreeMap::new();
    for (app, values) in records {
        let values = values
            .as_object()
            .context("Provider S2 app records must be an object")?;
        for (provider_id, payload) in values {
            output.insert((app.clone(), provider_id.clone()), payload.clone());
        }
    }
    Ok(output)
}

fn encrypted_account_payloads(
    root: Option<&Value>,
    accounts: &AccountStore,
) -> anyhow::Result<BTreeMap<(String, String), Value>> {
    if accounts.accounts.is_empty() {
        return Ok(BTreeMap::new());
    }
    let values = root
        .and_then(|root| root.get("accounts"))
        .and_then(Value::as_array)
        .context("accounts.json entries are missing")?;
    let mut output = BTreeMap::new();
    for payload in values {
        let provider_type = json_string(payload, &["providerType", "provider_type"])?;
        let id = json_string(payload, &["id"])?;
        ensure!(
            output
                .insert((provider_type, id), payload.clone())
                .is_none(),
            "duplicate Account source key"
        );
    }
    Ok(output)
}

fn share_payloads(
    root: Option<&Value>,
    shares: &ShareStore,
) -> anyhow::Result<BTreeMap<String, Value>> {
    if shares.shares.is_empty() {
        return Ok(BTreeMap::new());
    }
    let values = root
        .and_then(|root| root.get("shares"))
        .and_then(Value::as_array)
        .context("shares.json entries are missing")?;
    let mut output = BTreeMap::new();
    for payload in values {
        let id = json_string(payload, &["id"])?;
        ensure!(
            output.insert(id, payload.clone()).is_none(),
            "duplicate Share source key"
        );
    }
    Ok(output)
}

fn ensure_account_payload_secrets_are_encrypted(value: &Value) -> anyhow::Result<()> {
    fn visit(value: &Value, parent: Option<&str>) -> anyhow::Result<()> {
        match value {
            Value::Object(object) => {
                for (key, value) in object {
                    let compact = key
                        .chars()
                        .filter(|character| character.is_ascii_alphanumeric())
                        .map(|character| character.to_ascii_lowercase())
                        .collect::<String>();
                    let secret = matches!(
                        compact.as_str(),
                        "token"
                            | "key"
                            | "secret"
                            | "authorization"
                            | "proxyauthorization"
                            | "cookie"
                            | "password"
                            | "sessiontoken"
                            | "githubtoken"
                            | "copilottoken"
                            | "devicecode"
                            | "usercode"
                            | "codeverifier"
                            | "authorizationcode"
                            | "clientassertion"
                            | "machinetoken"
                            | "securityoauthtoken"
                            | "personaltoken"
                    ) || [
                        "accesstoken",
                        "refreshtoken",
                        "idtoken",
                        "apikey",
                        "clientsecret",
                        "kiroapikey",
                        "secretaccesskey",
                        "privatekey",
                        "signingkey",
                    ]
                    .iter()
                    .any(|suffix| compact.ends_with(suffix))
                        || parent.is_some_and(|parent| {
                            parent
                                .chars()
                                .filter(|character| character.is_ascii_alphanumeric())
                                .map(|character| character.to_ascii_lowercase())
                                .collect::<String>()
                                == "extraheaders"
                        });
                    if secret {
                        if let Some(text) = value.as_str().filter(|text| !text.is_empty()) {
                            ensure!(
                                text.starts_with("ccenc:v1:") || text.starts_with("ccenc:v2:"),
                                "refusing to place plaintext Account credentials in SQLite"
                            );
                        }
                    }
                    visit(value, Some(key))?;
                }
            }
            Value::Array(values) => {
                for value in values {
                    visit(value, parent)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    visit(value, None)
}

fn collect_sources(config_dir: &Path) -> anyhow::Result<Vec<SourceFile>> {
    let mut paths = Vec::new();
    for path in [
        providers_path(config_dir),
        crate::domain::accounts::store::accounts_path(config_dir),
        shares_path(config_dir),
    ] {
        if path.is_file() {
            paths.push(path);
        }
    }
    collect_files_recursively(config_dir, &usage_directory(config_dir), &mut paths)?;
    paths.sort();
    let mut sources = Vec::new();
    for path in paths {
        let relative = path
            .strip_prefix(config_dir)
            .context("source path escaped config directory")?;
        let relative_path = relative_path_string(relative)?;
        let bytes =
            fs::read(&path).with_context(|| format!("read migration source {}", path.display()))?;
        sources.push(SourceFile {
            relative_path,
            sha256: sha256_hex(&bytes),
            bytes,
        });
    }
    Ok(sources)
}

fn collect_files_recursively(
    root: &Path,
    path: &Path,
    output: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    ensure!(
        path.starts_with(root),
        "migration source escaped config directory"
    );
    if path.is_file() {
        output.push(path.to_path_buf());
        return Ok(());
    }
    let mut entries = fs::read_dir(path)
        .with_context(|| format!("read migration source directory {}", path.display()))?
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let candidate = entry.path();
        ensure!(
            !candidate.is_symlink(),
            "migration sources may not contain symlinks"
        );
        collect_files_recursively(root, &candidate, output)?;
    }
    Ok(())
}

fn aggregate_source_digest(sources: &[SourceFile]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"cc-switch-server:sqlite-shadow-sources:v1\0");
    for source in sources {
        digest.update((source.relative_path.len() as u64).to_be_bytes());
        digest.update(source.relative_path.as_bytes());
        digest.update((source.bytes.len() as u64).to_be_bytes());
        digest.update(source.sha256.as_bytes());
    }
    hex::encode(digest.finalize())
}

fn expected_provider_key_hash(providers: &ProviderStore) -> String {
    let mut rows = providers
        .providers
        .iter()
        .map(|stored| {
            format!(
                "{}\0{}\0{}\0{}\0{}",
                stored.app.as_str(),
                stored.provider.id,
                stored.provider_type.as_str(),
                stored.resource.revision,
                stored.resource.credential_generation
            )
        })
        .collect::<Vec<_>>();
    rows.sort();
    sha256_hex(rows.join("\n").as_bytes())
}

fn provider_key_hash(connection: &Connection) -> anyhow::Result<String> {
    query_key_hash(connection, "SELECT app,provider_id,provider_type,revision,credential_generation FROM providers ORDER BY app,provider_id")
}

fn expected_account_key_hash(accounts: &AccountStore) -> String {
    let mut rows = accounts
        .accounts
        .iter()
        .map(|account| {
            format!(
                "{}\0{}\0{}\0{}",
                account.provider_type.as_str(),
                account.id,
                account.auth_identity_generation,
                account.token_refresh_generation
            )
        })
        .collect::<Vec<_>>();
    rows.sort();
    sha256_hex(rows.join("\n").as_bytes())
}

fn account_key_hash(connection: &Connection) -> anyhow::Result<String> {
    let mut statement = connection.prepare("SELECT provider_type,account_id,auth_identity_generation,token_refresh_generation FROM accounts ORDER BY provider_type,account_id")?;
    let rows = statement.query_map([], |row| {
        Ok(format!(
            "{}\0{}\0{}\0{}",
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?
        ))
    })?;
    let rows = rows.collect::<Result<Vec<_>, _>>()?;
    Ok(sha256_hex(rows.join("\n").as_bytes()))
}

fn query_key_hash(connection: &Connection, sql: &str) -> anyhow::Result<String> {
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map([], |row| {
        Ok(format!(
            "{}\0{}\0{}\0{}\0{}",
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?
        ))
    })?;
    let rows = rows.collect::<Result<Vec<_>, _>>()?;
    Ok(sha256_hex(rows.join("\n").as_bytes()))
}

fn set_meta(transaction: &Transaction<'_>, key: &str, value: &str) -> anyhow::Result<()> {
    transaction.execute(
        "INSERT INTO meta(key,value) VALUES(?1,?2)",
        params![key, value],
    )?;
    Ok(())
}

fn read_json_if_exists(path: &Path) -> anyhow::Result<Option<Value>> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parse {}", path.display()))
        .map(Some)
}

fn json_string(value: &Value, keys: &[&str]) -> anyhow::Result<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::to_string)
        .context("required JSON identity field is missing")
}

fn safe_relative_path(value: &str) -> anyhow::Result<PathBuf> {
    let path = Path::new(value);
    ensure!(!path.is_absolute(), "legacy blob path must be relative");
    ensure!(
        path.components()
            .all(|component| matches!(component, std::path::Component::Normal(_))),
        "legacy blob path contains an unsafe component"
    );
    Ok(path.to_path_buf())
}

fn relative_path_string(path: &Path) -> anyhow::Result<String> {
    let value = path
        .to_str()
        .context("migration source path must be UTF-8")?;
    safe_relative_path(value)?;
    Ok(value.replace('\\', "/"))
}

fn temporary_database_path(config_dir: &Path) -> PathBuf {
    let suffix: [u8; 8] = rand::random();
    config_dir.join(format!(".{DATABASE_FILE_NAME}.{}.tmp", hex::encode(suffix)))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn to_sql_u64(value: u64) -> anyhow::Result<i64> {
    i64::try_from(value).context("value exceeds SQLite signed integer range")
}

fn to_sql_u128(value: u128) -> anyhow::Result<i64> {
    i64::try_from(value).context("timestamp exceeds SQLite signed integer range")
}

fn sync_file(path: &Path) -> anyhow::Result<()> {
    fs::OpenOptions::new()
        .read(true)
        .open(path)
        .with_context(|| format!("open {} for sync", path.display()))?
        .sync_all()
        .with_context(|| format!("sync {}", path.display()))
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

fn set_private_directory_permissions(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("chmod 0700 {}", path.display()))?;
    }
    Ok(())
}

fn now_ms() -> i64 {
    crate::infra::time::now_ms().min(i64::MAX as u128) as i64
}

use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::domain::accounts::store::UpsertAccountInput;
    use crate::domain::providers::model::ProviderType;
    use crate::domain::usage::store::{TokenUsage, UsageModelMetadata};

    fn temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "cc-switch-server-sqlite-{label}-{}",
            rand::random::<u64>()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn account_store_with_token(token: &str) -> AccountStore {
        let mut accounts = AccountStore::default();
        accounts.upsert(UpsertAccountInput {
            id: Some("account-1".to_string()),
            provider_type: ProviderType::ClaudeOAuth,
            email: Some("owner@example.com".to_string()),
            access_token: Some(token.to_string()),
            refresh_token: Some("refresh-must-not-enter-db".to_string()),
            id_token: None,
            token_type: Some("Bearer".to_string()),
            api_key: None,
            extra_headers: None,
            scopes: Vec::new(),
            profile: None,
            raw: None,
            subscription_level: None,
            entitlement_status: None,
            quota_percent: None,
            quota: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: None,
            rate_limited_until: None,
            last_refresh_error: None,
        });
        accounts
    }

    fn usage_log(sequence: usize) -> UsageLog {
        let mut log = UsageLog::new(
            crate::domain::providers::model::AppKind::Codex,
            "provider-1".to_string(),
            "Provider 1".to_string(),
            ProviderType::Codex,
            200,
            1,
            UsageModelMetadata::default(),
            TokenUsage {
                input_tokens: Some(1),
                output_tokens: Some(1),
                total_tokens: Some(2),
                ..Default::default()
            },
        );
        log.request_id = format!("request-{sequence:04}");
        log.created_at_ms = u128::try_from(sequence).unwrap().saturating_add(1);
        log
    }

    #[test]
    fn shadow_import_preserves_encrypted_account_bytes_and_exports_exactly() {
        let dir = temp_dir("roundtrip");
        let accounts = account_store_with_token("plaintext-must-not-enter-db");
        accounts.save(&dir).unwrap();
        let encrypted_source =
            fs::read(crate::domain::accounts::store::accounts_path(&dir)).unwrap();
        assert!(!String::from_utf8_lossy(&encrypted_source).contains("plaintext-must-not-enter-db"));
        let providers = ProviderStore::default();
        let shares = ShareStore::default();
        let usage = UsageStore::default();
        let report = refresh_shadow(
            &dir,
            ShadowImportInput {
                providers: &providers,
                accounts: &accounts,
                shares: &shares,
                usage: &usage,
            },
        )
        .unwrap();
        assert_eq!(report.account_count, 1);
        assert_eq!(report.credentials_verified, 1);
        let database_bytes = fs::read(database_path(&dir)).unwrap();
        assert!(!String::from_utf8_lossy(&database_bytes).contains("plaintext-must-not-enter-db"));

        let export = dir.with_extension("export");
        let count = export_legacy(&dir, &export).unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            fs::read(crate::domain::accounts::store::accounts_path(&export)).unwrap(),
            encrypted_source
        );
        let reloaded = AccountStore::load_or_default(&export).unwrap();
        assert_eq!(
            reloaded.accounts[0].access_token.as_deref(),
            Some("plaintext-must-not-enter-db")
        );

        fs::remove_dir_all(&dir).unwrap();
        fs::remove_dir_all(&export).unwrap();
    }

    #[test]
    fn failed_replacement_keeps_the_previous_verified_database_and_marker() {
        let dir = temp_dir("fault");
        let providers = ProviderStore::default();
        let accounts = AccountStore::default();
        let shares = ShareStore::default();
        let usage = UsageStore::default();
        let input = || ShadowImportInput {
            providers: &providers,
            accounts: &accounts,
            shares: &shares,
            usage: &usage,
        };
        refresh_shadow(&dir, input()).unwrap();
        let database_before = fs::read(database_path(&dir)).unwrap();
        let marker_before = fs::read(marker_path(&dir)).unwrap();
        let error = refresh_shadow_with_hook(&dir, input(), |stage| {
            if stage == ShadowImportStage::BeforeReplace {
                anyhow::bail!("injected before replace");
            }
            Ok(())
        })
        .unwrap_err();
        assert!(error.to_string().contains("injected before replace"));
        assert_eq!(fs::read(database_path(&dir)).unwrap(), database_before);
        assert_eq!(fs::read(marker_path(&dir)).unwrap(), marker_before);
        assert!(verify_shadow(&dir, input()).is_ok());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn online_backup_is_integrity_checked_and_exportable() {
        let dir = temp_dir("backup");
        let providers = ProviderStore::default();
        let accounts = AccountStore::default();
        let shares = ShareStore::default();
        let usage = UsageStore::default();
        refresh_shadow(
            &dir,
            ShadowImportInput {
                providers: &providers,
                accounts: &accounts,
                shares: &shares,
                usage: &usage,
            },
        )
        .unwrap();
        let destination = dir.join("backup.sqlite3");
        online_backup(&dir, &destination).unwrap();
        let connection = open_database(&destination, true).unwrap();
        let integrity: String = connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn source_digest_drift_invalidates_the_shadow_until_it_is_rebuilt() {
        let dir = temp_dir("source-drift");
        let providers = ProviderStore::default();
        let accounts = AccountStore::default();
        let shares = ShareStore::default();
        let usage = UsageStore::default();
        let input = || ShadowImportInput {
            providers: &providers,
            accounts: &accounts,
            shares: &shares,
            usage: &usage,
        };
        refresh_shadow(&dir, input()).unwrap();
        fs::write(shares_path(&dir), b"{\"shares\":[]}\n").unwrap();
        let error = verify_shadow(&dir, input()).unwrap_err();
        assert!(error
            .to_string()
            .contains("legacy stores changed after the SQLite shadow was built"));
        let rebuilt = ensure_shadow(&dir, input()).unwrap();
        assert_eq!(
            read_marker(&dir).unwrap().unwrap().source_digest,
            rebuilt.source_digest
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn schema_foreign_keys_reject_orphaned_share_graphs() {
        let dir = temp_dir("foreign-key");
        let path = dir.join("fixture.sqlite3");
        let connection = open_database(&path, false).unwrap();
        initialize_schema(&connection).unwrap();
        let error = connection
            .execute(
                "INSERT INTO shares(share_id,app,provider_id,provider_type,config_revision,payload_json,payload_sha256)
                 VALUES('share-1','claude','missing','claude_oauth',1,'{}','digest')",
                [],
            )
            .unwrap_err();
        assert!(error.to_string().contains("FOREIGN KEY constraint failed"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn staged_backup_requires_matching_database_marker_and_legacy_bytes() {
        let dir = temp_dir("backup-validate");
        fs::write(shares_path(&dir), b"{\"shares\":[]}\n").unwrap();
        let providers = ProviderStore::default();
        let accounts = AccountStore::default();
        let shares = ShareStore::default();
        let usage = UsageStore::default();
        refresh_shadow(
            &dir,
            ShadowImportInput {
                providers: &providers,
                accounts: &accounts,
                shares: &shares,
                usage: &usage,
            },
        )
        .unwrap();
        assert!(validate_backup_pair(&dir).is_ok());
        fs::write(shares_path(&dir), b"{\"shares\":[{\"id\":\"tampered\"}]}\n").unwrap();
        let error = validate_backup_pair(&dir).unwrap_err();
        assert!(error
            .to_string()
            .contains("staged SQLite source does not match legacy blob"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn committed_authority_retires_legacy_and_roundtrips_account_writes() {
        let dir = temp_dir("authority-roundtrip");
        let accounts = account_store_with_token("first-secret");
        accounts.save(&dir).unwrap();
        let providers = ProviderStore::default();
        let shares = ShareStore::default();
        let usage = UsageStore::default();
        refresh_shadow(
            &dir,
            ShadowImportInput {
                providers: &providers,
                accounts: &accounts,
                shares: &shares,
                usage: &usage,
            },
        )
        .unwrap();
        let marker = activate_authority(&dir).unwrap();
        assert_eq!(marker.authority, AuthorityState::Committed);
        assert!(!crate::domain::accounts::store::accounts_path(&dir).exists());
        assert!(dir
            .join(marker.legacy_backup_dir.as_deref().unwrap())
            .join("accounts.json")
            .is_file());

        let mut loaded = load_authoritative(&dir).unwrap();
        assert_eq!(
            loaded.accounts.accounts[0].access_token.as_deref(),
            Some("first-secret")
        );
        loaded.accounts.accounts[0].access_token = Some("rotated-secret".to_string());
        loaded.accounts.accounts[0].token_refresh_generation += 1;
        persist_accounts(&dir, &loaded.accounts).unwrap();
        let reloaded = load_authoritative(&dir).unwrap();
        assert_eq!(
            reloaded.accounts.accounts[0].access_token.as_deref(),
            Some("rotated-secret")
        );
        let database_bytes = fs::read(database_path(&dir)).unwrap();
        assert!(!String::from_utf8_lossy(&database_bytes).contains("rotated-secret"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn every_authority_transition_crash_point_rolls_forward() {
        for stage in [
            AuthorityTransitionStage::LegacyBackupCreated,
            AuthorityTransitionStage::DatabasePrepared,
            AuthorityTransitionStage::MarkerPrepared,
            AuthorityTransitionStage::DatabaseCommitted,
            AuthorityTransitionStage::MarkerCommitted,
            AuthorityTransitionStage::LegacyRetired,
        ] {
            let dir = temp_dir(&format!("transition-{stage:?}"));
            let accounts = account_store_with_token("crash-safe-secret");
            accounts.save(&dir).unwrap();
            let providers = ProviderStore::default();
            let shares = ShareStore::default();
            let usage = UsageStore::default();
            refresh_shadow(
                &dir,
                ShadowImportInput {
                    providers: &providers,
                    accounts: &accounts,
                    shares: &shares,
                    usage: &usage,
                },
            )
            .unwrap();
            let error = activate_authority_with_hook(&dir, |observed| {
                if observed == stage {
                    anyhow::bail!("injected transition crash")
                }
                Ok(())
            })
            .unwrap_err();
            assert!(error.to_string().contains("injected transition crash"));
            let marker = activate_authority(&dir).unwrap();
            assert_eq!(marker.authority, AuthorityState::Committed);
            assert_eq!(
                load_authoritative(&dir).unwrap().accounts.accounts[0]
                    .access_token
                    .as_deref(),
                Some("crash-safe-secret")
            );
            assert!(!crate::domain::accounts::store::accounts_path(&dir).exists());
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn committed_authority_ignores_and_retires_reintroduced_legacy_files() {
        let dir = temp_dir("no-reverse-import");
        let accounts = account_store_with_token("database-secret");
        accounts.save(&dir).unwrap();
        let providers = ProviderStore::default();
        let shares = ShareStore::default();
        let usage = UsageStore::default();
        refresh_shadow(
            &dir,
            ShadowImportInput {
                providers: &providers,
                accounts: &accounts,
                shares: &shares,
                usage: &usage,
            },
        )
        .unwrap();
        activate_authority(&dir).unwrap();
        fs::write(
            crate::domain::accounts::store::accounts_path(&dir),
            b"{malformed legacy must not win",
        )
        .unwrap();
        assert_eq!(
            load_authoritative(&dir).unwrap().accounts.accounts[0]
                .access_token
                .as_deref(),
            Some("database-secret")
        );
        activate_authority(&dir).unwrap();
        assert!(!crate::domain::accounts::store::accounts_path(&dir).exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stale_account_generation_cannot_overwrite_a_newer_sqlite_commit() {
        let dir = temp_dir("account-generation-cas");
        let accounts = account_store_with_token("generation-zero");
        accounts.save(&dir).unwrap();
        let providers = ProviderStore::default();
        let shares = ShareStore::default();
        let usage = UsageStore::default();
        refresh_shadow(
            &dir,
            ShadowImportInput {
                providers: &providers,
                accounts: &accounts,
                shares: &shares,
                usage: &usage,
            },
        )
        .unwrap();
        activate_authority(&dir).unwrap();

        let mut stale = load_authoritative(&dir).unwrap().accounts;
        stale.accounts[0].token_refresh_generation = 1;
        stale.accounts[0].access_token = Some("stale-token".to_string());
        let mut current = stale.clone();
        current.accounts[0].token_refresh_generation = 2;
        current.accounts[0].access_token = Some("current-token".to_string());
        persist_accounts(&dir, &current).unwrap();
        let error = persist_accounts(&dir, &stale).unwrap_err();
        assert!(error
            .to_string()
            .contains("stale Account identity/token generation"));
        let reloaded = load_authoritative(&dir).unwrap();
        assert_eq!(
            reloaded.accounts.accounts[0].access_token.as_deref(),
            Some("current-token")
        );
        assert_eq!(reloaded.accounts.accounts[0].token_refresh_generation, 2);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stale_share_snapshot_cannot_overwrite_or_delete_newer_rows() {
        let dir = temp_dir("share-revision-cas");
        let providers = ProviderStore::default();
        let accounts = AccountStore::default();
        let shares = ShareStore::default();
        let usage = UsageStore::default();
        refresh_shadow(
            &dir,
            ShadowImportInput {
                providers: &providers,
                accounts: &accounts,
                shares: &shares,
                usage: &usage,
            },
        )
        .unwrap();
        activate_authority(&dir).unwrap();

        let connection = open_database(&database_path(&dir), false).unwrap();
        let provider_payload = "{}";
        connection
            .execute(
                "INSERT INTO providers(app,provider_id,provider_type,revision,credential_generation,payload_json,payload_sha256)
                 VALUES('claude','provider-1','claude_oauth',2,1,?1,?2)",
                params![provider_payload, sha256_hex(provider_payload.as_bytes())],
            )
            .unwrap();
        let share_payload = "{}";
        connection
            .execute(
                "INSERT INTO shares(share_id,app,provider_id,provider_type,config_revision,payload_json,payload_sha256)
                 VALUES('share-1','claude','provider-1','claude_oauth',2,?1,?2)",
                params![share_payload, sha256_hex(share_payload.as_bytes())],
            )
            .unwrap();
        drop(connection);

        let error = persist_shares(&dir, &ShareStore::default()).unwrap_err();
        assert!(error
            .to_string()
            .contains("attempted deletion without a tombstone"));
        let connection = open_database(&database_path(&dir), true).unwrap();
        let count: i64 = connection
            .query_row("SELECT count(*) FROM shares", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count, 1,
            "rejected stale commit must have no partial effect"
        );
        drop(connection);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sqlite_full_fault_rolls_back_account_transaction_without_losing_authority() {
        let dir = temp_dir("sqlite-full");
        let accounts = account_store_with_token("before-full");
        accounts.save(&dir).unwrap();
        let providers = ProviderStore::default();
        let shares = ShareStore::default();
        let usage = UsageStore::default();
        refresh_shadow(
            &dir,
            ShadowImportInput {
                providers: &providers,
                accounts: &accounts,
                shares: &shares,
                usage: &usage,
            },
        )
        .unwrap();
        activate_authority(&dir).unwrap();

        let mut enlarged = load_authoritative(&dir).unwrap().accounts;
        let original_generation = enlarged.accounts[0].token_refresh_generation;
        enlarged.accounts[0].token_refresh_generation = original_generation.saturating_add(1);
        enlarged.accounts[0].access_token = Some("must-not-commit".to_string());
        enlarged.accounts[0].raw = Some(json!({"padding": "x".repeat(4096)}));
        let error = persist_accounts_with_hook(&dir, &enlarged, || {
            Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
                Some("database or disk is full".to_string()),
            )
            .into())
        })
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("database or disk is full"),
            "unexpected failure: {error:#}"
        );
        let reloaded = load_authoritative(&dir).unwrap();
        assert_eq!(
            reloaded.accounts.accounts[0].access_token.as_deref(),
            Some("before-full")
        );
        assert_eq!(
            reloaded.accounts.accounts[0].token_refresh_generation,
            original_generation
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn online_backup_is_consistent_during_usage_upserts() {
        let dir = temp_dir("concurrent-usage-backup");
        let providers = ProviderStore::default();
        let accounts = AccountStore::default();
        let shares = ShareStore::default();
        let usage = UsageStore::default();
        refresh_shadow(
            &dir,
            ShadowImportInput {
                providers: &providers,
                accounts: &accounts,
                shares: &shares,
                usage: &usage,
            },
        )
        .unwrap();
        activate_authority(&dir).unwrap();

        let completed = Arc::new(AtomicUsize::new(0));
        let writer_completed = Arc::clone(&completed);
        let writer_dir = dir.clone();
        let writer = std::thread::spawn(move || {
            for sequence in 0..256 {
                upsert_usage_log(&writer_dir, &usage_log(sequence)).unwrap();
                writer_completed.store(sequence + 1, Ordering::Release);
            }
        });
        while completed.load(Ordering::Acquire) == 0 {
            std::thread::yield_now();
        }
        let backup = dir.join("concurrent-backup.sqlite3");
        online_backup(&dir, &backup).unwrap();
        writer.join().unwrap();

        let connection = open_database(&backup, true).unwrap();
        let integrity: String = connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
        validate_payload_digests(&connection).unwrap();
        let count: i64 = connection
            .query_row("SELECT count(*) FROM usage_records", [], |row| row.get(0))
            .unwrap();
        assert!((1..=256).contains(&count));
        drop(connection);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn staged_restore_rejects_tampered_semantic_rollback_payload() {
        let dir = temp_dir("semantic-rollback-payload");
        let providers = ProviderStore::default();
        let accounts = AccountStore::default();
        let shares = ShareStore::default();
        shares.save(&dir).unwrap();
        let usage = UsageStore::default();
        refresh_shadow(
            &dir,
            ShadowImportInput {
                providers: &providers,
                accounts: &accounts,
                shares: &shares,
                usage: &usage,
            },
        )
        .unwrap();
        activate_authority(&dir).unwrap();

        let invalid = br#"{"shares":[{"id":"orphaned"}]}"#;
        let connection = open_database(&database_path(&dir), false).unwrap();
        connection
            .execute(
                "UPDATE legacy_blobs SET payload=?1,sha256=?2,byte_length=?3 WHERE relative_path='shares.json'",
                params![invalid, sha256_hex(invalid), i64::try_from(invalid.len()).unwrap()],
            )
            .unwrap();
        drop(connection);
        let error = validate_backup_pair(&dir).unwrap_err();
        assert!(
            format!("{error:#}").contains("load Shares while validating SQLite graph"),
            "unexpected validation error: {error:#}"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn committed_online_backup_validates_without_legacy_source_files() {
        let dir = temp_dir("committed-backup");
        let accounts = account_store_with_token("backup-secret");
        accounts.save(&dir).unwrap();
        let providers = ProviderStore::default();
        let shares = ShareStore::default();
        let usage = UsageStore::default();
        refresh_shadow(
            &dir,
            ShadowImportInput {
                providers: &providers,
                accounts: &accounts,
                shares: &shares,
                usage: &usage,
            },
        )
        .unwrap();
        activate_authority(&dir).unwrap();
        let stage = dir.join("backup-stage");
        fs::create_dir_all(&stage).unwrap();
        online_backup(&dir, &database_path(&stage)).unwrap();
        fs::copy(marker_path(&dir), marker_path(&stage)).unwrap();
        fs::copy(
            crate::domain::accounts::store::accounts_key_path(&dir),
            crate::domain::accounts::store::accounts_key_path(&stage),
        )
        .unwrap();
        assert!(validate_backup_pair(&stage).is_ok());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrupt_committed_database_fails_closed() {
        let dir = temp_dir("committed-corruption");
        let accounts = account_store_with_token("corruption-secret");
        accounts.save(&dir).unwrap();
        let providers = ProviderStore::default();
        let shares = ShareStore::default();
        let usage = UsageStore::default();
        refresh_shadow(
            &dir,
            ShadowImportInput {
                providers: &providers,
                accounts: &accounts,
                shares: &shares,
                usage: &usage,
            },
        )
        .unwrap();
        activate_authority(&dir).unwrap();
        fs::write(database_path(&dir), b"not a sqlite database").unwrap();
        assert!(load_authoritative(&dir).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrupt_nonempty_wal_fails_closed_before_authoritative_load() {
        let dir = temp_dir("wal-corruption");
        let providers = ProviderStore::default();
        let accounts = AccountStore::default();
        let shares = ShareStore::default();
        let usage = UsageStore::default();
        refresh_shadow(
            &dir,
            ShadowImportInput {
                providers: &providers,
                accounts: &accounts,
                shares: &shares,
                usage: &usage,
            },
        )
        .unwrap();
        activate_authority(&dir).unwrap();
        fs::write(
            database_path(&dir).with_extension("sqlite3-wal"),
            b"corrupt committed WAL",
        )
        .unwrap();

        let error = match load_authoritative(&dir) {
            Ok(_) => panic!("corrupt WAL must not be accepted"),
            Err(error) => error,
        };
        assert!(
            format!("{error:#}").contains("WAL"),
            "unexpected corruption error: {error:#}"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn wal_checksum_validation_accepts_complete_frames_and_rejects_bit_flips() {
        use std::io::{Read as _, Seek, SeekFrom, Write as _};

        let dir = temp_dir("wal-checksum");
        let providers = ProviderStore::default();
        let accounts = AccountStore::default();
        let shares = ShareStore::default();
        let usage = UsageStore::default();
        refresh_shadow(
            &dir,
            ShadowImportInput {
                providers: &providers,
                accounts: &accounts,
                shares: &shares,
                usage: &usage,
            },
        )
        .unwrap();
        activate_authority(&dir).unwrap();

        let database = database_path(&dir);
        let connection = open_database(&database, false).unwrap();
        connection
            .execute(
                "UPDATE meta SET value='1' WHERE key='repository_generation'",
                [],
            )
            .unwrap();
        validate_wal_if_present(&database).unwrap();

        let wal = database.with_file_name(format!(
            "{}-wal",
            database.file_name().unwrap().to_str().unwrap()
        ));
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&wal)
            .unwrap();
        let last = file.metadata().unwrap().len() - 1;
        file.seek(SeekFrom::Start(last)).unwrap();
        let mut byte = [0_u8; 1];
        file.read_exact(&mut byte).unwrap();
        file.seek(SeekFrom::Start(last)).unwrap();
        file.write_all(&[byte[0] ^ 0x01]).unwrap();
        file.sync_all().unwrap();
        let error = validate_wal_if_present(&database).unwrap_err();
        assert!(error.to_string().contains("WAL checksum mismatch"));

        drop(file);
        drop(connection);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn newer_marker_schema_is_rejected_by_downgrade_binary() {
        let dir = temp_dir("downgrade-marker");
        fs::write(
            marker_path(&dir),
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": SCHEMA_VERSION + 1,
                "authority": "committed",
                "sourceDigest": "fixture",
                "databaseFile": DATABASE_FILE_NAME,
                "preparedAtMs": 1,
                "committedAtMs": 2
            }))
            .unwrap(),
        )
        .unwrap();
        let error = read_marker(&dir).unwrap_err();
        assert!(error
            .to_string()
            .contains("unsupported Server SQLite marker schema"));
        fs::remove_dir_all(dir).unwrap();
    }
}
