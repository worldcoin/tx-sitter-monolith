-- Fail instead of queueing every query on the table behind a long lock wait
SET LOCAL lock_timeout = '5s';

-- The L1 fees that each OP-stack chain's GasPriceOracle reported at the last
-- successful read, which a starting worker resumes L1 fee gating from.
-- Expand-only: binaries that predate L1 fee gating don't read it.
CREATE TABLE IF NOT EXISTS l1_fees (
    chain_id      BIGINT PRIMARY KEY,
    l1_base_fee   BYTEA NOT NULL,
    blob_base_fee BYTEA NOT NULL,
    read_at       TIMESTAMPTZ NOT NULL
);
