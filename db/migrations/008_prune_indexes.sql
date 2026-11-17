-- Indexes on the columns that pruning deletes through: deleting a transaction
-- finds its hashes by tx_id, each deleted hash is looked up in
-- sent_transactions.valid_tx_hash, and deleting a block finds its block_txs.
--
-- Building an index blocks writes to its table until it is built. On a large
-- table, build it before deploying with CREATE INDEX CONCURRENTLY and the same
-- name and columns; this migration then skips it. Check that such an index is
-- valid, since a failed concurrent build leaves an invalid one behind.

-- Fail instead of queueing every query on the table behind a long lock wait
SET LOCAL lock_timeout = '5s';

CREATE INDEX IF NOT EXISTS tx_hashes_tx_id ON tx_hashes (tx_id);
CREATE INDEX IF NOT EXISTS sent_transactions_valid_tx_hash ON sent_transactions (valid_tx_hash);
CREATE INDEX IF NOT EXISTS block_txs_block ON block_txs (block_number, chain_id);
