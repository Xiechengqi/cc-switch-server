-- Financial consumption evidence is independent of request-log retention.
CREATE TABLE market_settlement_requests (
    request_id TEXT PRIMARY KEY,
    share_id TEXT NOT NULL,
    entitlement_id TEXT NOT NULL,
    email TEXT NOT NULL,
    phase TEXT NOT NULL,
    started_at_ms INTEGER NOT NULL,
    revision INTEGER NOT NULL CHECK(revision>=0),
    tokens TEXT NOT NULL,
    value_picos TEXT NOT NULL,
    final_state TEXT NOT NULL DEFAULT 'pending' CHECK(final_state IN('pending','uncertain','final')),
    no_value_execution INTEGER NOT NULL DEFAULT 0 CHECK(no_value_execution IN(0,1)),
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json))
) STRICT;
CREATE INDEX market_settlement_entitlement ON market_settlement_requests(share_id,entitlement_id,started_at_ms);
CREATE TABLE market_settlement_receipts (
    operation_id TEXT PRIMARY KEY,
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json))
) STRICT;
CREATE TRIGGER market_receipt_immutable_update BEFORE UPDATE ON market_settlement_receipts
BEGIN SELECT RAISE(ABORT,'immutable Market settlement receipt'); END;
CREATE TRIGGER market_receipt_immutable_delete BEFORE DELETE ON market_settlement_receipts
BEGIN SELECT RAISE(ABORT,'immutable Market settlement receipt'); END;
-- Keep successful ACKs until Router acceptance AND local marker cleanup are
-- durable. Router may stop returning an edit before the Server sees its ACK.
CREATE TABLE market_settlement_ack_outbox (
    operation_id TEXT PRIMARY KEY REFERENCES market_settlement_receipts(operation_id),
    router_api_base TEXT NOT NULL,
    installation_id TEXT NOT NULL,
    share_id TEXT NOT NULL,
    termination_id TEXT NOT NULL,
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    queued_at_ms INTEGER NOT NULL,
    last_attempt_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX market_settlement_ack_target ON market_settlement_ack_outbox(router_api_base,installation_id,last_attempt_at_ms,operation_id);
CREATE TRIGGER market_ack_payload_immutable BEFORE UPDATE ON market_settlement_ack_outbox
WHEN NEW.operation_id IS NOT OLD.operation_id OR NEW.router_api_base IS NOT OLD.router_api_base
 OR NEW.installation_id IS NOT OLD.installation_id OR NEW.share_id IS NOT OLD.share_id
 OR NEW.termination_id IS NOT OLD.termination_id OR NEW.payload_json IS NOT OLD.payload_json
 OR NEW.queued_at_ms IS NOT OLD.queued_at_ms
BEGIN SELECT RAISE(ABORT,'immutable Market settlement ACK'); END;
INSERT INTO meta(key,value) VALUES('market_settlement_since_ms',CAST(CAST((julianday('now')-2440587.5)*86400000 AS INTEGER) AS TEXT));
