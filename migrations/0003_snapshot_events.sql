-- Worker messages reported by the rig runtime (HiveOS-style "Messages").
CREATE TABLE machine_events(
  id uuid PRIMARY KEY,
  machine_id uuid NOT NULL REFERENCES machines ON DELETE CASCADE,
  seq bigint NOT NULL,
  at timestamptz NOT NULL,
  level text NOT NULL,
  kind text NOT NULL,
  instance text,
  message text NOT NULL,
  detail jsonb
);
CREATE INDEX machine_events_machine_time ON machine_events(machine_id, at DESC);
CREATE INDEX audit_events_target_time ON audit_events(target, created_at DESC);
-- latest_observations is rewritten every 10 s per machine and kind: leave room for HOT updates
-- and vacuum it far more often than the default 20% dead-tuple threshold.
ALTER TABLE latest_observations SET (fillfactor = 70, autovacuum_vacuum_scale_factor = 0.02, autovacuum_analyze_scale_factor = 0.05);
