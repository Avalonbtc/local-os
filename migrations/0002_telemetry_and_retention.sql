-- Forward-only migration: keep task/deployment history when retiring a machine.
ALTER TABLE machines ADD COLUMN deleted_at timestamptz;
CREATE INDEX deployment_versions_machine_time ON deployment_versions(machine_id,created_at DESC);
CREATE INDEX job_targets_machine ON job_targets(machine_id);
CREATE TABLE telemetry_watermarks(name text PRIMARY KEY, through_at timestamptz NOT NULL);

-- Strip inventory and protocol payloads BEFORE returning or persisting samples.
CREATE FUNCTION metric_values(k text, d jsonb) RETURNS jsonb LANGUAGE sql IMMUTABLE AS $$
SELECT CASE WHEN jsonb_typeof(d) <> 'object' THEN d
 WHEN k='system' THEN d - ARRAY['topology','cpu_model','hostname','os','kernel','architecture','board','bios','interfaces','network_interfaces','hardware','dmi','numa','runtime_version']
 WHEN k='sensors' THEN jsonb_build_object('items',COALESCE((SELECT jsonb_agg(jsonb_build_object('name',x->'name','unit',x->'unit','reading',x->'reading','health',x->'health','raw',jsonb_build_object('PowerConsumedWatts',x->'raw'->'PowerConsumedWatts'))) FROM jsonb_array_elements(COALESCE(d->'items','[]')) x),'[]'))
 ELSE d END
$$;
CREATE FUNCTION create_metric_partitions() RETURNS void LANGUAGE plpgsql AS $$
DECLARE d date; tab text;
BEGIN
 FOR d IN SELECT generate_series(current_date-7,current_date+14,interval '1 day')::date LOOP
  tab := 'metrics_' || to_char(d,'YYYYMMDD');
  IF to_regclass(tab) IS NULL THEN
   EXECUTE format('CREATE TABLE IF NOT EXISTS %I PARTITION OF metrics FOR VALUES FROM (%L) TO (%L)',tab,d::timestamptz,(d+1)::timestamptz);
  END IF;
 END LOOP;
END $$;
-- Original function remains callable but no longer couples DROP with CREATE.
CREATE OR REPLACE FUNCTION maintain_partitions() RETURNS void LANGUAGE sql AS $$ SELECT create_metric_partitions() $$;
SELECT create_metric_partitions();
