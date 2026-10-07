-- A lost SSH connection no longer parks a target in 'unknown' (which kept the machine locked
-- until someone resolved it by hand). The worker re-queries the rig's operation record itself,
-- backing off between attempts.
ALTER TABLE job_targets ADD COLUMN retry_at timestamptz, ADD COLUMN attempts int NOT NULL DEFAULT 0;

-- Rig-side operations are recorded under their operation id, so re-querying one only reads
-- that record. Hand every target already waiting for a person back to the worker.
UPDATE job_targets t SET status='reconciling', lease=NULL, lease_until=NULL, retry_at=now()
FROM jobs j
WHERE j.id=t.job_id AND t.status='unknown'
  AND j.action->>'kind' IN ('apply','miner','command','adopt','gpu_oc','gpu_oc_reset','bootstrap');
UPDATE jobs j SET status='running'
WHERE j.status='unknown' AND EXISTS(SELECT 1 FROM job_targets t WHERE t.job_id=j.id AND t.status='reconciling');
