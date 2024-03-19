ALTER TABLE relayers
ADD COLUMN max_queued_txs BIGINT NOT NULL DEFAULT 20,
ADD CONSTRAINT check_max_queued_txs CHECK (max_queued_txs > max_inflight_txs);

UPDATE relayers
SET max_queued_txs = GREATEST(max_inflight_txs, 20);
