-- Farm-wide preferences (currently the electricity price for the daily cost estimate).
-- One row; settings live in `data` so later preferences need no schema change.
CREATE TABLE farm_settings(
    id boolean PRIMARY KEY DEFAULT true CHECK (id),
    data jsonb NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
