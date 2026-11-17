-- Fail instead of queueing every query on the table behind a long lock wait
SET LOCAL lock_timeout = '5s';

-- Expand-only: binaries that predate the v2 API insert transactions without
-- these columns, which their defaults cover. Rows that exist when this runs
-- get the migration time as their creation time.
--
-- metadata is the caller's JSON text exactly as sent. It is TEXT rather than
-- JSON or JSONB so that Postgres neither rewrites it nor rejects JSON the API
-- accepted.
ALTER TABLE transactions
ADD COLUMN IF NOT EXISTS metadata     TEXT,
ADD COLUMN IF NOT EXISTS request_hash BYTEA,
ADD COLUMN IF NOT EXISTS created_at   TIMESTAMPTZ NOT NULL DEFAULT now();
