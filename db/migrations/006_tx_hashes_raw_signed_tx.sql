-- The signed transaction as sent with eth_sendRawTransaction, so that it can
-- be sent again. NULL for hashes stored before this column was added.
ALTER TABLE tx_hashes ADD COLUMN raw_signed_tx BYTEA;
