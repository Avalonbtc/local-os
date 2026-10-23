-- Offline detection: the evaluated status per machine (a MachineStatus document, so later fields
-- need no schema change) and the offline/recovery alerts raised when it changes.
CREATE TABLE machine_status(
    machine_id uuid PRIMARY KEY REFERENCES machines ON DELETE CASCADE,
    data jsonb NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE status_alerts(
    id bigserial PRIMARY KEY,
    machine_id uuid NOT NULL REFERENCES machines ON DELETE CASCADE,
    kind text NOT NULL CHECK (kind IN ('local','pool')),
    event text NOT NULL CHECK (event IN ('offline','recovered')),
    at timestamptz NOT NULL,
    message text NOT NULL
);
CREATE INDEX status_alerts_at ON status_alerts(at);
