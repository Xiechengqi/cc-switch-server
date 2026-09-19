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
        .context("initialize Server SQLite schema")
}
