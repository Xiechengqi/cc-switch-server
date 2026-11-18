CREATE TABLE IF NOT EXISTS share_capability_admissions (
    request_id TEXT NOT NULL,
    attempt INTEGER NOT NULL,
    scope TEXT NOT NULL,
    binding TEXT NOT NULL,
    plan_json TEXT NOT NULL CHECK(json_valid(plan_json)),
    admitted_at_ms INTEGER NOT NULL,
    PRIMARY KEY(request_id,attempt)
);
CREATE INDEX IF NOT EXISTS share_capability_admissions_time ON share_capability_admissions(admitted_at_ms);
CREATE TABLE IF NOT EXISTS share_capability_contexts (
    scope TEXT NOT NULL,
    binding TEXT NOT NULL,
    response_id TEXT NOT NULL,
    plan_json TEXT NOT NULL CHECK(json_valid(plan_json)),
    expires_at_ms INTEGER NOT NULL,
    PRIMARY KEY(scope,binding,response_id)
);
CREATE INDEX IF NOT EXISTS share_capability_contexts_expiry ON share_capability_contexts(expires_at_ms);
CREATE TABLE IF NOT EXISTS share_capability_incidents (
    scope TEXT NOT NULL,
    binding TEXT NOT NULL,
    request_id TEXT NOT NULL,
    resource TEXT NOT NULL,
    detected_at_ms INTEGER NOT NULL,
    PRIMARY KEY(scope,binding,request_id,resource)
);

-- Durable settings receipts share the same transaction as the canonical Share.
-- Pruning the in-document replay window must not replay an expired capability preview.
CREATE TABLE IF NOT EXISTS share_settings_receipts (
    router_url TEXT NOT NULL,
    installation_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    share_id TEXT NOT NULL,
    receipt_json TEXT NOT NULL CHECK(json_valid(receipt_json)),
    PRIMARY KEY(router_url,installation_id,operation_id)
);
