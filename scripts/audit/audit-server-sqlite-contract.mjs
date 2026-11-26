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
const schema = read("src/repository/server_sqlite/schema.rs");
const payload = read("src/repository/server_sqlite/payload.rs");
const wal = read("src/repository/server_sqlite/wal.rs");
const state = read("src/state.rs");
const cli = read("src/cli.rs");
const admin = read("src/admin.rs");
const metrics = read("src/metrics.rs");
const storageDoc = read("docs/architecture/storage.md");

for (const moduleName of ["payload", "schema", "wal"]) {
  assert(
    repository.includes(`mod ${moduleName};`),
    `SQLite repository must declare its private ${moduleName} module`,
  );
}

for (const fragment of [
  "PRAGMA foreign_keys=ON",
  "PRAGMA trusted_schema=OFF",
  "PRAGMA journal_mode=WAL",
  "PRAGMA synchronous=FULL",
  "PRAGMA integrity_check",
  "foreign_key_check",
  "validate_wal_if_present",
  "refusing to rebuild a committed SQLite authority",
]) {
  assert(repository.includes(fragment), `SQLite repository contract lost: ${fragment}`);
}
for (const fragment of [
  "BEGIN IMMEDIATE",
  "CREATE TABLE providers",
  "CREATE TABLE accounts",
  "CREATE TABLE provider_accounts",
  "CREATE TABLE shares",
  "CREATE TABLE share_bindings",
  "CREATE TABLE usage_records",
  "CREATE TABLE legacy_blobs",
]) {
  assert(schema.includes(fragment), `SQLite schema contract lost: ${fragment}`);
}
assert(
  payload.includes("refusing to place plaintext Account credentials in SQLite"),
  "SQLite encrypted-payload guard is missing",
);
assert(
  wal.includes("validate_wal_if_present") &&
    wal.includes("SQLite WAL checksum mismatch"),
  "SQLite WAL envelope validation is missing",
);

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
    state.includes("server_sqlite::load_runtime") &&
    state.includes("create_consistent_backup") &&
    state.includes("server_sqlite::validate_backup_pair"),
  "Server startup/backup/restore no longer closes the SQLite shadow loop",
);
const history = read("src/repository/server_sqlite/history.rs");
const usageStore = read("src/domain/usage/store.rs");
assert(
  history.includes("recover_interrupted") && history.includes("quota::upsert_log") &&
  history.includes("transaction.commit()") && !history.includes("DELETE FROM usage_records"),
  "startup recovery must commit affected Usage revisions and their exact accounting incrementally",
);
assert(
  repository.includes("a recent Usage cache cannot replace durable history") &&
  usageStore.includes("CACHE_LIMIT") && usageStore.includes("CACHE_BYTES") &&
  history.includes("BEGIN DEFERRED") && state.includes("acquire_owned"),
  "bounded Usage caching, query snapshot, or cancellation-safe concurrency is missing",
);
const querySchema = read("src/repository/server_sqlite/usage_query_schema.sql");
for (const index of ["usage_provider_statistics_idx", "usage_probe_identity_idx", "usage_share_export_idx", "usage_pending_export_idx"]) {
  assert(querySchema.includes(`CREATE INDEX ${index}`), `missing bounded query index: ${index}`);
}
assert(
  schema.includes("usage_query_schema_v1") && schema.includes("Usage query extension checksum mismatch") &&
  !querySchema.includes("PRAGMA user_version") && !querySchema.includes("DROP TABLE"),
  "query projections must install atomically without changing the v10 authority format",
);
assert(
  history.includes("router_export_sequence > ?") && history.includes("selection.after_export_sequence") &&
  history.includes("LIMIT ?") && history.includes("INDEXED BY usage_provider_statistics_idx") &&
  history.includes("INDEXED BY usage_share_export_idx") && history.includes("INDEXED BY usage_pending_export_idx"),
  "export cursors must precede LIMIT and Provider statistics must use the covering projection",
);
const health = read("src/domain/health.rs");
const snapshotStart = health.indexOf("pub fn provider_health_snapshot_for_plan(");
const snapshotEnd = health.indexOf("fn provider_health_with_runtime(", snapshotStart);
assert(snapshotStart >= 0 && !/visit_logs|select_logs|provider_statistics/.test(health.slice(snapshotStart, snapshotEnd)),
  "transport/scheduling health must not read request history");
const registrationStart = state.indexOf("pub async fn complete_router_registration_control_plane(");
const registrationEnd = state.indexOf("async fn register_router_installation_locked(", registrationStart);
assert(!/reconcile_all_shares_to_router\(|retry_pending_router_share_deletes\(/.test(state.slice(registrationStart, registrationEnd)),
  "Client registration completion must not wait for full Share reconciliation");
assert(usageStore.includes("pub(crate) fn read_snapshot(") &&
  usageStore.includes("provider_health: self.provider_health.clone()") &&
  usageStore.includes("quota_ledger: self.quota_ledger.clone()"),
  "durable read views must preserve health and quota metadata");
const quotaLedger = read("src/domain/usage/quota_ledger.rs");
const validateLogStart = quotaLedger.indexOf("pub fn validate_log(");
const validateLogEnd = quotaLedger.indexOf("pub fn record(", validateLogStart);
assert(
  !/self\s*\.entries\s*\.iter\s*\(/.test(quotaLedger.slice(validateLogStart, validateLogEnd)),
  "quota validation must use the request scope index on a new request",
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
