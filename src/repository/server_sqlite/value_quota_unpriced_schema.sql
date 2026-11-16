-- Owner-managed USD metering records a missing price as an explicit $0
-- attempt instead of refusing the request. NULL is an ordinary priced row;
-- every reason except usage_unobserved is re-priced when a covering tariff
-- activates inside a window that has not ended.
ALTER TABLE value_quota_attempts ADD COLUMN unpriced_reason TEXT CHECK(unpriced_reason IS NULL OR unpriced_reason IN
  ('model_unpriced','model_unresolved','tier_unpriced','resource_unpriced','tariff_not_effective','usage_unobserved'));
CREATE INDEX value_quota_unpriced_idx ON value_quota_attempts(scope,started_at_ms,request_id,attempt_id)
WHERE unpriced_reason IS NOT NULL;
