-- HiveOS-style overclocking templates. `data` holds the per-vendor, per-card lists (GpuOcConfig);
-- the values actually applied are pinned in each job's action, so editing a template never
-- changes what a rig is running.
CREATE TABLE oc_profiles(id uuid PRIMARY KEY, name text NOT NULL, data jsonb NOT NULL, revision int NOT NULL DEFAULT 1);
