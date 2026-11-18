-- Sweeper (pending/unavailable), terminal-history compaction and retention
-- scans select attempts by metering status and start time; without this
-- index each pass scanned the whole, ever-growing attempts table.
CREATE INDEX value_quota_status_time_idx ON value_quota_attempts(metering_status,started_at_ms,request_id,attempt_id);

-- Finished revisions are compacted in bounded pages. Already compacted rows
-- must not be scanned again on every heartbeat.
CREATE INDEX value_quota_uncompacted_time_idx ON value_quota_attempts(metering_status,started_at_ms,request_id,attempt_id) WHERE compacted_at_ms IS NULL;

-- Calendar admission may move the monetary clock across a boundary. Account
-- cycles must always use the immutable source clock, also when a calendar
-- grant is later converted to account-reset. A virtual column exposes that
-- clock without rewriting the evidence JSON or its integrity hash.
ALTER TABLE quota_metering_facts ADD COLUMN account_started_at_ms INTEGER
  GENERATED ALWAYS AS (COALESCE(json_extract(fact_json,'$.attribution.startedAtMs'),started_at_ms)) VIRTUAL;
DROP INDEX quota_facts_account_idx;
CREATE INDEX quota_facts_account_idx ON quota_metering_facts(scope,account_key,account_started_at_ms,request_id,attempt_id);
