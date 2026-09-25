CREATE TABLE users(id uuid PRIMARY KEY, name text NOT NULL UNIQUE, password_hash text NOT NULL, created_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE sessions(token_hash text PRIMARY KEY, user_id uuid NOT NULL REFERENCES users, csrf text NOT NULL, expires_at timestamptz NOT NULL, created_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE api_tokens(id uuid PRIMARY KEY, user_id uuid NOT NULL REFERENCES users, name text NOT NULL, token_hash text NOT NULL UNIQUE, created_at timestamptz NOT NULL DEFAULT now(), revoked_at timestamptz);
CREATE TABLE login_limits(name text PRIMARY KEY, attempts int NOT NULL, window_start timestamptz NOT NULL);
CREATE TABLE machines(id uuid PRIMARY KEY, name text NOT NULL UNIQUE, host text NOT NULL, port int NOT NULL CHECK(port BETWEEN 1 AND 65535), username text NOT NULL, host_key text NOT NULL, "group" text NOT NULL DEFAULT '', tags jsonb NOT NULL DEFAULT '[]', is_controller boolean NOT NULL DEFAULT false, bmc jsonb, policy jsonb NOT NULL DEFAULT '{}', ssh_secret text NOT NULL, bmc_secret text, created_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE coins(id uuid PRIMARY KEY, name text NOT NULL, data jsonb NOT NULL, revision int NOT NULL DEFAULT 1, symbol text GENERATED ALWAYS AS (upper(data->>'symbol')) STORED UNIQUE NOT NULL);
CREATE TABLE wallets(id uuid PRIMARY KEY, name text NOT NULL, coin_id uuid NOT NULL REFERENCES coins, data jsonb NOT NULL, revision int NOT NULL DEFAULT 1);
CREATE TABLE pools(id uuid PRIMARY KEY, name text NOT NULL, data jsonb NOT NULL, revision int NOT NULL DEFAULT 1);
CREATE TABLE miners(id uuid PRIMARY KEY, name text NOT NULL, data jsonb NOT NULL, revision int NOT NULL DEFAULT 1);
CREATE TABLE flight_sheets(id uuid PRIMARY KEY, name text NOT NULL UNIQUE, version int NOT NULL DEFAULT 1);
CREATE TABLE flight_tasks(sheet_id uuid NOT NULL REFERENCES flight_sheets ON DELETE CASCADE, instance text NOT NULL, ordinal int NOT NULL, miner_id uuid NOT NULL REFERENCES miners, wallet_id uuid NOT NULL REFERENCES wallets, pool_id uuid REFERENCES pools, config jsonb NOT NULL DEFAULT '{}', PRIMARY KEY(sheet_id,instance));
CREATE TABLE jobs(id uuid PRIMARY KEY, actor_id uuid NOT NULL REFERENCES users, actor text NOT NULL, action jsonb NOT NULL, request_hash text NOT NULL, idempotency_key text NOT NULL, status text NOT NULL DEFAULT 'queued', concurrency int NOT NULL CHECK(concurrency BETWEEN 1 AND 8), canary boolean NOT NULL, cancel_requested boolean NOT NULL DEFAULT false, created_at timestamptz NOT NULL DEFAULT now(), UNIQUE(actor_id,idempotency_key));
CREATE TABLE job_targets(id uuid PRIMARY KEY, job_id uuid NOT NULL REFERENCES jobs ON DELETE CASCADE, machine_id uuid NOT NULL REFERENCES machines, machine_name text NOT NULL, ordinal int NOT NULL, operation_id uuid NOT NULL UNIQUE, status text NOT NULL DEFAULT 'queued', lease uuid, lease_until timestamptz, output text NOT NULL DEFAULT '', output_truncated boolean NOT NULL DEFAULT false, result jsonb, error text, started_at timestamptz, finished_at timestamptz, UNIQUE(job_id,machine_id));
CREATE INDEX job_targets_queue ON job_targets(status,ordinal);
CREATE TABLE machine_locks(machine_id uuid PRIMARY KEY REFERENCES machines, target_id uuid NOT NULL UNIQUE REFERENCES job_targets, acquired_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE deployment_versions(id uuid PRIMARY KEY, job_id uuid NOT NULL REFERENCES jobs, machine_id uuid NOT NULL REFERENCES machines, snapshot jsonb NOT NULL, created_at timestamptz NOT NULL DEFAULT now(), UNIQUE(job_id,machine_id));
CREATE TABLE latest_observations(machine_id uuid NOT NULL REFERENCES machines ON DELETE CASCADE, kind text NOT NULL, observed_at timestamptz NOT NULL, data jsonb NOT NULL, error text, PRIMARY KEY(machine_id,kind));
CREATE TABLE metrics(machine_id uuid NOT NULL REFERENCES machines ON DELETE CASCADE, kind text NOT NULL, observed_at timestamptz NOT NULL, data jsonb NOT NULL, error text) PARTITION BY RANGE(observed_at);
CREATE INDEX metrics_host_time ON metrics(machine_id,kind,observed_at DESC);
CREATE TABLE metric_minutes(machine_id uuid NOT NULL REFERENCES machines ON DELETE CASCADE, kind text NOT NULL, observed_at timestamptz NOT NULL, data jsonb NOT NULL, error text, PRIMARY KEY(machine_id,kind,observed_at));
CREATE TABLE audit_events(id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, actor text NOT NULL, action text NOT NULL, target text NOT NULL, detail jsonb NOT NULL DEFAULT '{}', created_at timestamptz NOT NULL DEFAULT now());
CREATE INDEX audit_time ON audit_events(created_at DESC);
CREATE TABLE package_artifacts(sha256 text PRIMARY KEY CHECK(length(sha256)=64), filename text NOT NULL, size_bytes bigint NOT NULL, created_at timestamptz NOT NULL DEFAULT now());

CREATE FUNCTION maintain_partitions() RETURNS void LANGUAGE plpgsql AS $$
DECLARE d date; tab text; row record;
BEGIN
  FOR d IN SELECT generate_series(current_date - 7,current_date + 2,interval '1 day')::date LOOP
    tab := 'metrics_' || to_char(d,'YYYYMMDD');
    EXECUTE format('CREATE TABLE IF NOT EXISTS %I PARTITION OF metrics FOR VALUES FROM (%L) TO (%L)',tab,d::timestamptz,(d+1)::timestamptz);
  END LOOP;
  FOR row IN SELECT c.relname FROM pg_inherits i JOIN pg_class c ON c.oid=i.inhrelid WHERE i.inhparent='metrics'::regclass LOOP
    IF row.relname ~ '^metrics_[0-9]{8}$' AND to_date(substring(row.relname from 9),'YYYYMMDD') < current_date-7 THEN
      EXECUTE format('DROP TABLE %I',row.relname);
    END IF;
  END LOOP;
END $$;
SELECT maintain_partitions();

