-- Fail instead of queueing every query on the table behind a long lock wait
SET LOCAL lock_timeout = '5s';

-- The revision and hash of the static config the database applied last.
-- Expand-only: binaries that predate the static config don't read it.
CREATE TABLE IF NOT EXISTS static_config_version (
    -- Allows a single row
    singleton  BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    revision   BIGINT NOT NULL,
    hash       BYTEA NOT NULL,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
