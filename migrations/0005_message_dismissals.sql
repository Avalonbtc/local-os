-- HiveOS-style message chips: closing a chip (or clearing all with the bin) only hides it from
-- the worker header; the full activity history is untouched.
CREATE TABLE message_clears(
    machine_id uuid PRIMARY KEY REFERENCES machines ON DELETE CASCADE,
    cleared_at timestamptz NOT NULL
);
CREATE TABLE message_dismissals(
    machine_id uuid NOT NULL REFERENCES machines ON DELETE CASCADE,
    message_key text NOT NULL,
    dismissed_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(machine_id, message_key)
);
