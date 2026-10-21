-- Rigs join by running one install command and keep a connection open to the controller
-- (HiveOS style). BMC and BIOS management are gone: a public controller cannot reach the
-- farms' management networks.
ALTER TABLE machines DROP COLUMN bmc, DROP COLUMN bmc_secret;
ALTER TABLE machines
    ADD COLUMN transport text NOT NULL DEFAULT 'ssh' CHECK (transport IN ('ssh','agent')),
    ADD COLUMN fingerprint text CHECK (fingerprint ~ '^[0-9a-f]{64}$');
-- A reinstalled rig (same /etc/machine-id) finds its machine again; a deleted one starts over.
CREATE UNIQUE INDEX machines_fingerprint ON machines(fingerprint) WHERE deleted_at IS NULL;
DELETE FROM latest_observations WHERE kind IN ('power','sensors','events');

CREATE TABLE enrollment_tokens(
    id uuid PRIMARY KEY,
    name text NOT NULL,
    "group" text NOT NULL DEFAULT '',
    tags jsonb NOT NULL DEFAULT '[]',
    expires_at timestamptz,
    max_uses int CHECK (max_uses > 0),
    uses int NOT NULL DEFAULT 0,
    revoked_at timestamptz,
    created_by text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
