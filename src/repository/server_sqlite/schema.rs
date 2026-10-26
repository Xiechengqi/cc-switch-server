use anyhow::Context;
use rusqlite::Connection;

pub(super) fn initialize_schema(connection: &Connection) -> anyhow::Result<()> {
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
        .context("initialize Server SQLite schema")?;
    migrate_to_current(connection)
}

/// v1 is immutable. This transaction can be retried after a crash before the
/// separate authority marker was updated; it never reimports legacy files.
pub(super) fn migrate_to_current(connection: &Connection) -> anyhow::Result<()> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    anyhow::ensure!((1..=2).contains(&version),
        "unsupported Server SQLite schema {version}; use a matching Server binary/backup (never downgrade)");
    let checksum: String = connection.query_row(
        "SELECT checksum FROM schema_migrations WHERE version=1",
        [],
        |row| row.get(0),
    )?;
    anyhow::ensure!(
        checksum == "sha256:server-store-schema-v1",
        "Server SQLite v1 migration checksum mismatch"
    );
    if version == 1 {
        connection
            .execute_batch(
                "BEGIN IMMEDIATE;
             CREATE TABLE quota_usage_entries(
               request_id TEXT NOT NULL,
               usage_revision INTEGER NOT NULL CHECK(usage_revision >= 0),
               watermark INTEGER NOT NULL UNIQUE CHECK(watermark > 0),
               source_ref TEXT NOT NULL,
               source_epoch INTEGER NOT NULL,
               bucket_id TEXT NOT NULL,
               share_id TEXT NOT NULL,
               user_email TEXT NOT NULL,
               started_at_ms INTEGER NOT NULL CHECK(started_at_ms >= 0),
               payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
               payload_sha256 TEXT NOT NULL,
               PRIMARY KEY(request_id,usage_revision)
             ) STRICT;
             CREATE INDEX quota_usage_scope_start_idx ON quota_usage_entries(
               source_ref,source_epoch,bucket_id,share_id,user_email,started_at_ms,watermark);
             INSERT INTO schema_migrations(version,name,applied_at_ms,checksum)
             VALUES(2,'exact_account_quota_usage',0,'sha256:server-store-schema-v2-quota');
             PRAGMA user_version=2;
             COMMIT;",
            )
            .context("migrate Server SQLite v1 to v2")?;
    }
    let checksum: String = connection.query_row(
        "SELECT checksum FROM schema_migrations WHERE version=2",
        [],
        |row| row.get(0),
    )?;
    anyhow::ensure!(
        checksum == "sha256:server-store-schema-v2-quota",
        "Server SQLite v2 migration checksum mismatch"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_v1_migration_is_idempotent_and_preserves_original_checksum() {
        let connection = Connection::open_in_memory().unwrap();
        initialize_schema(&connection).unwrap();
        connection.execute_batch("DROP TABLE quota_usage_entries; DELETE FROM schema_migrations WHERE version=2; PRAGMA user_version=1;").unwrap();
        connection
            .execute(
                "INSERT INTO meta(key,value) VALUES('existing','preserved')",
                [],
            )
            .unwrap();
        migrate_to_current(&connection).unwrap();
        migrate_to_current(&connection).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row("SELECT value FROM meta WHERE key='existing'", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "preserved"
        );
    }

    #[test]
    fn quota_higher_schema_or_changed_migration_is_never_downgraded() {
        let connection = Connection::open_in_memory().unwrap();
        initialize_schema(&connection).unwrap();
        connection.execute_batch("PRAGMA user_version=3;").unwrap();
        assert!(migrate_to_current(&connection)
            .unwrap_err()
            .to_string()
            .contains("never downgrade"));
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            3
        );
        connection.execute_batch("PRAGMA user_version=2; UPDATE schema_migrations SET checksum='modified' WHERE version=1;").unwrap();
        assert!(migrate_to_current(&connection).is_err());
    }
}
