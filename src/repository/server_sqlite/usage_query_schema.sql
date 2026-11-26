-- Read projections stay derived from the digest-checked authoritative payload.
-- Indexed virtual columns need no second writer or revision/backfill journal.
ALTER TABLE usage_records ADD COLUMN app TEXT GENERATED ALWAYS AS (json_extract(payload_json,'$.app')) VIRTUAL;
ALTER TABLE usage_records ADD COLUMN is_health_check INTEGER GENERATED ALWAYS AS (coalesce(json_extract(payload_json,'$.isHealthCheck'),0)) VIRTUAL;
ALTER TABLE usage_records ADD COLUMN record_kind TEXT GENERATED ALWAYS AS (coalesce(json_extract(payload_json,'$.recordKind'),'user_inference')) VIRTUAL;
ALTER TABLE usage_records ADD COLUMN data_source TEXT GENERATED ALWAYS AS (json_extract(payload_json,'$.dataSource')) VIRTUAL;
ALTER TABLE usage_records ADD COLUMN health_fingerprint TEXT GENERATED ALWAYS AS (json_extract(payload_json,'$.providerHealthFingerprint')) VIRTUAL;
ALTER TABLE usage_records ADD COLUMN requested_model TEXT GENERATED ALWAYS AS (json_extract(payload_json,'$.requestedModel')) VIRTUAL;
ALTER TABLE usage_records ADD COLUMN status_code INTEGER GENERATED ALWAYS AS (json_extract(payload_json,'$.statusCode')) VIRTUAL;
ALTER TABLE usage_records ADD COLUMN duration_ms_text TEXT GENERATED ALWAYS AS (coalesce(payload_json -> '$.durationMs','0')) VIRTUAL;
ALTER TABLE usage_records ADD COLUMN router_export_sequence INTEGER GENERATED ALWAYS AS (coalesce(json_extract(payload_json,'$.routerExportSequence'),0)) VIRTUAL;
ALTER TABLE usage_records ADD COLUMN router_last_synced_at_ms INTEGER GENERATED ALWAYS AS (json_extract(payload_json,'$.routerLastSyncedAtMs')) VIRTUAL;
ALTER TABLE usage_records ADD COLUMN router_next_sync_attempt_at_ms INTEGER GENERATED ALWAYS AS (coalesce(json_extract(payload_json,'$.routerNextSyncAttemptAtMs'),0)) VIRTUAL;

CREATE INDEX usage_provider_statistics_idx ON usage_records(provider_id,app,is_health_check,created_at_ms,status_code,duration_ms_text);
CREATE INDEX usage_probe_identity_idx ON usage_records(provider_id,app,is_health_check,share_id,data_source,health_fingerprint,requested_model,created_at_ms,request_id);
CREATE INDEX usage_provider_recent_idx ON usage_records(provider_id,app,is_health_check,created_at_ms,request_id);
CREATE INDEX usage_share_export_idx ON usage_records(share_id,router_export_sequence,created_at_ms,request_id)
 WHERE record_kind='user_inference' AND data_source='router_share' AND is_health_check=0;
CREATE INDEX usage_pending_export_idx ON usage_records(router_export_sequence,created_at_ms,request_id,router_next_sync_attempt_at_ms)
 WHERE router_last_synced_at_ms IS NULL AND share_id IS NOT NULL
 AND record_kind='user_inference' AND data_source='router_share' AND is_health_check=0;
