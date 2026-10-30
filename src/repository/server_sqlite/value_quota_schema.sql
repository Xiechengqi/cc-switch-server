-- Monetary equivalents are quota counters, never wallet postings.
CREATE TABLE value_pricing_snapshots (
  router_url TEXT NOT NULL,
  installation_id TEXT NOT NULL,
  revision TEXT NOT NULL,
  payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
  installed_at_ms INTEGER NOT NULL CHECK(installed_at_ms >= 0),
  PRIMARY KEY(router_url,installation_id,revision)
) STRICT;
CREATE TABLE value_quota_coverage (
  scope TEXT PRIMARY KEY,
  collected_from_ms INTEGER NOT NULL CHECK(collected_from_ms >= 0)
) STRICT;
CREATE TABLE value_quota_attempts (
  request_id TEXT NOT NULL,
  attempt_id TEXT NOT NULL,
  scope TEXT NOT NULL,
  policy_epoch TEXT NOT NULL,
  window_id TEXT NOT NULL,
  started_at_ms INTEGER NOT NULL CHECK(started_at_ms >= 0),
  pricing_revision TEXT NOT NULL,
  effective_model TEXT NOT NULL,
  effective_tier TEXT NOT NULL,
  frozen_tariff_json TEXT NOT NULL CHECK(json_valid(frozen_tariff_json)),
  usage_revision INTEGER NOT NULL CHECK(usage_revision >= 0),
  usage_picos TEXT NOT NULL,
  pending_risk_picos TEXT NOT NULL,
  metering_status TEXT NOT NULL CHECK(metering_status IN ('pending','observed','conservative','unavailable')),
  observation_sha256 TEXT,
  compacted_at_ms INTEGER CHECK(compacted_at_ms >= 0),
  watermark INTEGER NOT NULL CHECK(watermark > 0),
  PRIMARY KEY(request_id,attempt_id)
) STRICT;
CREATE INDEX value_quota_scope_time_idx ON value_quota_attempts(scope,started_at_ms,request_id,attempt_id);
-- Exact UTC-day checkpoints; boundary fragments are read from minimal attempts.
-- Every mutation updates its checkpoint in the same IMMEDIATE transaction.
CREATE TABLE value_quota_checkpoints (
  scope TEXT NOT NULL,
  day_start_ms INTEGER NOT NULL CHECK(day_start_ms >= 0),
  payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
  payload_sha256 TEXT NOT NULL,
  PRIMARY KEY(scope,day_start_ms)
) STRICT;
CREATE TABLE value_quota_gaps (
  scope TEXT NOT NULL,
  request_id TEXT NOT NULL,
  started_at_ms INTEGER NOT NULL CHECK(started_at_ms >= 0),
  reason TEXT NOT NULL,
  watermark INTEGER NOT NULL CHECK(watermark>0),
  PRIMARY KEY(scope,request_id)
) STRICT;
CREATE TABLE value_quota_revisions (
  request_id TEXT NOT NULL,
  attempt_id TEXT NOT NULL,
  usage_revision INTEGER NOT NULL CHECK(usage_revision >= 0),
  observation_sha256 TEXT NOT NULL,
  observation_json TEXT NOT NULL CHECK(json_valid(observation_json)),
  usage_picos TEXT NOT NULL,
  metering_status TEXT NOT NULL,
  watermark INTEGER NOT NULL UNIQUE CHECK(watermark > 0),
  PRIMARY KEY(request_id,attempt_id,usage_revision),
  FOREIGN KEY(request_id,attempt_id) REFERENCES value_quota_attempts(request_id,attempt_id)
) STRICT;
CREATE TABLE value_quota_adjustments (
  operation_id TEXT PRIMARY KEY,
  scope TEXT NOT NULL,
  window_id TEXT NOT NULL,
  payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
  target_picos TEXT,
  observed_watermark INTEGER NOT NULL,
  actor TEXT NOT NULL,
  reason TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL
  ,watermark INTEGER NOT NULL UNIQUE CHECK(watermark>0)
) STRICT;
CREATE INDEX value_quota_adjustment_window_idx ON value_quota_adjustments(scope,window_id,created_at_ms,operation_id);
INSERT INTO meta(key,value) VALUES('value_quota_watermark_v1','0');
