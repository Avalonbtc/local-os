-- Flight sheets carry the miner (download URL, version) and pool URLs inline, like HiveOS.
-- Rigs download miners themselves; the miners/pools tables are kept only as history.
ALTER TABLE flight_tasks ADD COLUMN miner jsonb;
UPDATE flight_tasks t
   SET miner = m.data || jsonb_build_object('name', COALESCE(NULLIF(m.data->>'custom_name', ''), m.name))
  FROM miners m
 WHERE m.id = t.miner_id;
-- A chosen pool becomes its server list, unless the task already picked servers from it.
UPDATE flight_tasks t
   SET config = t.config || jsonb_build_object('urls', p.data->'urls')
  FROM pools p
 WHERE p.id = t.pool_id
   AND CASE WHEN jsonb_typeof(t.config->'urls') = 'array' THEN jsonb_array_length(t.config->'urls') = 0 ELSE true END;
-- Old miner entries carried defaults (pool URLs, algorithm, template...) used when the task did
-- not set them. Copy them into the task so re-saving the sheet keeps the same behaviour.
UPDATE flight_tasks t
   SET config = jsonb_strip_nulls(jsonb_build_object(
         'urls', CASE WHEN jsonb_typeof(t.miner->'urls') = 'array' THEN t.miner->'urls' END,
         'algorithm', t.miner->'algorithm',
         'wallet_template', t.miner->'wallet_template',
         'password', t.miner->'password',
         'user_config', t.miner->'user_config')) || t.config;
ALTER TABLE flight_tasks ALTER COLUMN miner SET NOT NULL;
ALTER TABLE flight_tasks DROP COLUMN miner_id, DROP COLUMN pool_id;
