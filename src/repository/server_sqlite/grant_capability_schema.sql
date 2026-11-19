-- Old attempts may have only a price estimate. Preserve their evidence as-is.
ALTER TABLE value_quota_attempts ADD COLUMN quota_enforced INTEGER
    CHECK(quota_enforced IS NULL OR quota_enforced IN (0,1));
