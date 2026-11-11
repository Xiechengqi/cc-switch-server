-- Evidence outlives report retention and ordinary grant revocation. Never
-- attach these tables to shares: Share snapshots replace that table wholesale.
CREATE TABLE quota_metering_facts (
  scope TEXT NOT NULL,
  request_id TEXT NOT NULL,
  attempt_id TEXT NOT NULL,
  started_at_ms INTEGER NOT NULL CHECK(started_at_ms >= 0),
  accounted_at_ms INTEGER,
  account_key TEXT,
  usage_revision INTEGER NOT NULL CHECK(usage_revision >= 0),
  quota_tokens INTEGER NOT NULL CHECK(quota_tokens >= 0),
  request_count INTEGER NOT NULL CHECK(request_count IN (0,1)),
  fact_json TEXT NOT NULL CHECK(json_valid(fact_json)),
  fact_sha256 TEXT NOT NULL,
  PRIMARY KEY(request_id,attempt_id)
) STRICT;
CREATE INDEX quota_facts_account_idx ON quota_metering_facts(scope,account_key,started_at_ms,request_id,attempt_id);
CREATE INDEX quota_facts_scope_request_idx ON quota_metering_facts(scope,request_id,attempt_id);
CREATE INDEX quota_facts_scope_start_idx ON quota_metering_facts(scope,started_at_ms,request_id,attempt_id);
CREATE INDEX quota_facts_scope_accounted_idx ON quota_metering_facts(scope,accounted_at_ms,request_id,attempt_id);
CREATE INDEX usage_records_quota_backfill_idx ON usage_records(share_id,lower(json_extract(payload_json,'$.userEmail')),request_id);
CREATE INDEX quota_facts_live_idx ON quota_metering_facts(scope,started_at_ms)
WHERE json_extract(fact_json,'$.outcome')='inProgress' OR (accounted_at_ms IS NULL AND json_extract(fact_json,'$.outcome') IN ('final','finalConservative'));
CREATE TABLE quota_history_preparations (
  scope TEXT PRIMARY KEY,
  cursor TEXT NOT NULL,
  pricing_revision TEXT NOT NULL,
  state TEXT NOT NULL CHECK(state IN ('preparing','complete')),
  processed INTEGER NOT NULL CHECK(processed >= 0),
  updated_at_ms INTEGER NOT NULL
) STRICT;
CREATE TABLE quota_conversion_receipts (
  operation_id TEXT NOT NULL,
  scope TEXT NOT NULL,
  window_id TEXT NOT NULL,
  watermark INTEGER NOT NULL UNIQUE CHECK(watermark>0),
  payload_sha256 TEXT NOT NULL,
  receipt_json TEXT NOT NULL CHECK(json_valid(receipt_json)),
  settings_receipt_json TEXT CHECK(settings_receipt_json IS NULL OR json_valid(settings_receipt_json)),
  created_at_ms INTEGER NOT NULL,
  PRIMARY KEY(operation_id,scope)
) STRICT;
CREATE INDEX quota_conversion_window_idx ON quota_conversion_receipts(scope,window_id,watermark);
CREATE TABLE quota_adjustment_mappings (
  scope TEXT NOT NULL,
  window_id TEXT NOT NULL,
  source_adjustment TEXT NOT NULL,
  target_unit TEXT NOT NULL CHECK(target_unit IN ('tokens','usdEquivalent')),
  target_offset TEXT NOT NULL,
  reason TEXT NOT NULL,
  actor TEXT NOT NULL,
  operation_id TEXT NOT NULL,
  PRIMARY KEY(scope,window_id,source_adjustment,target_unit)
) STRICT;
CREATE TABLE quota_shadow_intervals (
  scope TEXT PRIMARY KEY,
  value_history_through_ms INTEGER NOT NULL CHECK(value_history_through_ms >= 0),
  metered_from_ms INTEGER NOT NULL CHECK(metered_from_ms >= 0)
) STRICT;

-- Unpriced Token generations still need durable async ownership. Identity is
-- checked against either the evidence fact or the original priced attempt.
ALTER TABLE value_async_tasks RENAME TO value_async_tasks_v5;
CREATE TABLE value_async_tasks (
  scope TEXT NOT NULL,
  task_id TEXT NOT NULL,
  request_id TEXT NOT NULL,
  attempt_id TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('pending','completed','failed')),
  expires_at_ms INTEGER NOT NULL CHECK(expires_at_ms>=0),
  observation_sha256 TEXT,
  PRIMARY KEY(scope,task_id),
  UNIQUE(request_id,attempt_id)
) STRICT;
INSERT INTO value_async_tasks SELECT * FROM value_async_tasks_v5;
DROP TABLE value_async_tasks_v5;
CREATE INDEX value_async_pending_idx ON value_async_tasks(status,expires_at_ms);
