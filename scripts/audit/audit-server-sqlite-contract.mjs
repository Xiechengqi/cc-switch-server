#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const read = (relative) => fs.readFileSync(path.join(root, relative), "utf8");
const assert = (condition, message) => {
  if (!condition) throw new Error(message);
};

const repository = read("src/repository/server_sqlite.rs");
const state = read("src/state.rs");
const cli = read("src/cli.rs");
const admin = read("src/admin.rs");
const metrics = read("src/metrics.rs");
const storageDoc = read("docs/architecture/storage.md");

for (const fragment of [
  "PRAGMA foreign_keys=ON",
  "PRAGMA trusted_schema=OFF",
  "PRAGMA journal_mode=WAL",
  "PRAGMA synchronous=FULL",
  "BEGIN IMMEDIATE",
  "CREATE TABLE providers",
  "CREATE TABLE accounts",
  "CREATE TABLE provider_accounts",
  "CREATE TABLE shares",
  "CREATE TABLE share_bindings",
  "CREATE TABLE usage_records",
  "CREATE TABLE legacy_blobs",
  "PRAGMA integrity_check",
  "foreign_key_check",
  "validate_wal_if_present",
  "SQLite WAL checksum mismatch",
  "refusing to place plaintext Account credentials in SQLite",
  "refusing to rebuild a committed SQLite authority",
]) {
  assert(repository.includes(fragment), `SQLite repository contract lost: ${fragment}`);
}

assert(
  repository.includes("rusqlite::backup::Backup::new"),
  "SQLite backups must use the online backup API",
);
assert(
  repository.includes("validate_authoritative_graph") &&
    repository.includes("subscription_reference_graph_errors") &&
    repository.includes("validate_share_revision_transition"),
  "staged restore graph validation or Share revision CAS is incomplete",
);
assert(
  cli.includes("MigrateServerStore") &&
    cli.includes("rollback_export") &&
    admin.includes("migrate_server_store_result") &&
    admin.includes("export_legacy"),
  "offline SQLite apply/rollback-export CLI is incomplete",
);
assert(
  metrics.includes("cc_switch_server_sqlite_commits_total") &&
    metrics.includes("cc_switch_server_sqlite_commit_duration_seconds"),
  "SQLite commit metrics are missing",
);
const usageUpsertStart = repository.indexOf("pub(crate) fn upsert_usage_log");
const usageUpsertEnd = repository.indexOf("\npub(crate) fn ", usageUpsertStart + 1);
const usageUpsert = repository.slice(usageUpsertStart, usageUpsertEnd);
assert(
  state.includes("upsert_usage_log") &&
    usageUpsert.includes("upsert_usage_row") &&
    repository.includes("ON CONFLICT(request_id)") &&
    !usageUpsert.includes("checkpoint_database") &&
    !usageUpsert.includes("integrity_check") &&
    !usageUpsert.includes("validate_payload_digests"),
  "Usage hot-path UPSERT must not checkpoint or scan the full database",
);
assert(
  repository.includes("accounts_key_path(config_dir)") &&
    repository.includes("accounts_key_path(destination_dir)"),
  "DB-to-legacy export must require and copy the external root key",
);
assert(
  repository.includes("activate_authority_with_hook") &&
    repository.includes("load_authoritative") &&
    repository.includes("persist_reference_graph") &&
    repository.includes("upsert_usage_log"),
  "SQLite committed reader/writer and crash-resume state machine are incomplete",
);
assert(
  state.includes("server_sqlite::ensure_shadow") &&
    state.includes("server_sqlite::activate_authority") &&
    state.includes("server_sqlite::load_authoritative") &&
    state.includes("create_consistent_backup") &&
    state.includes("server_sqlite::validate_backup_pair"),
  "Server startup/backup/restore no longer closes the SQLite shadow loop",
);
for (const fragment of [
  "shadow_verified",
  "prepared",
  "committed",
  "accounts.key` **不进入 SQLite**",
  "SQLite 是唯一权威",
  "只读 migration backup",
  "migrate-server-store --apply",
  "--rollback-export",
]) {
  assert(storageDoc.includes(fragment), `storage authority documentation lost: ${fragment}`);
}

console.log("Server SQLite authority contract ok (schema, crash recovery, transactional writers, backup, rollback)");
