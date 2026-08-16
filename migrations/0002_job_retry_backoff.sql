-- Retry scheduling for transient download failures.
--
-- Jobs that fail for a recoverable reason (provider hiccup, expired signed URL,
-- dropped connection) are returned to the queue with `next_attempt_at` set to a
-- future timestamp instead of being failed terminally. `claim_next` skips queued
-- jobs whose backoff has not elapsed, so a retrying job never starves the queue.
ALTER TABLE jobs ADD COLUMN next_attempt_at INTEGER;

DROP INDEX IF EXISTS idx_jobs_state_created_at;

CREATE INDEX idx_jobs_state_next_attempt_at ON jobs(state, next_attempt_at, created_at);
