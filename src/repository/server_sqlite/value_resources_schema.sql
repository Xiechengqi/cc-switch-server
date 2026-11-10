-- Durable async ownership links outlive response bodies and process restarts.
CREATE TABLE value_async_tasks (
  scope TEXT NOT NULL,
  task_id TEXT NOT NULL,
  request_id TEXT NOT NULL,
  attempt_id TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('pending','completed','failed')),
  expires_at_ms INTEGER NOT NULL CHECK(expires_at_ms>=0),
  observation_sha256 TEXT,
  PRIMARY KEY(scope,task_id),
  UNIQUE(request_id,attempt_id),
  FOREIGN KEY(request_id,attempt_id) REFERENCES value_quota_attempts(request_id,attempt_id)
) STRICT;
CREATE INDEX value_async_pending_idx ON value_async_tasks(status,expires_at_ms);
