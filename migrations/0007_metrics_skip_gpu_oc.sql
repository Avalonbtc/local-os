-- The overclock summary (profile, per-card results) and driver versions ride along in every 10 s
-- system snapshot. They are only needed in latest_observations; keep them out of the metrics
-- history and minute aggregates, which store one copy per sample.
CREATE OR REPLACE FUNCTION metric_values(k text, d jsonb) RETURNS jsonb LANGUAGE sql IMMUTABLE AS $$
SELECT CASE WHEN jsonb_typeof(d) <> 'object' THEN d
 WHEN k='system' THEN d - ARRAY['topology','cpu_model','hostname','os','kernel','architecture','board','bios','interfaces','network_interfaces','hardware','dmi','numa','runtime_version','gpu_oc','gpu_drivers']
 WHEN k='sensors' THEN jsonb_build_object('items',COALESCE((SELECT jsonb_agg(jsonb_build_object('name',x->'name','unit',x->'unit','reading',x->'reading','health',x->'health','raw',jsonb_build_object('PowerConsumedWatts',x->'raw'->'PowerConsumedWatts'))) FROM jsonb_array_elements(COALESCE(d->'items','[]')) x),'[]'))
 ELSE d END
$$;
