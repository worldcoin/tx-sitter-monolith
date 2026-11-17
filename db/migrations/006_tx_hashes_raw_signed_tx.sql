-- Fail instead of queueing every query on the table behind a long lock wait
SET LOCAL lock_timeout = '5s';

-- The signed transaction as sent with eth_sendRawTransaction, so that it can
-- be sent again. NULL for hashes stored before this column was added, and for
-- versions that an escalation replaced.
ALTER TABLE tx_hashes ADD COLUMN IF NOT EXISTS raw_signed_tx BYTEA;
