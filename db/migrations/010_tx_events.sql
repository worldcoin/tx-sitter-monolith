-- Fail instead of queueing every query on the table behind a long lock wait
SET LOCAL lock_timeout = '5s';

-- One row per status change of a transaction, written in the transaction that
-- makes the change. The feed serves rows in (tx_xid, id) order, and only those
-- of transactions older than every running one, so that a row committed late
-- can't land behind a position a consumer already read past.
--
-- relayer_id is VARCHAR rather than CHAR(36) like relayers.id: a CHAR column
-- compared with a text parameter is cast to text, which keeps the feed's
-- index from being used.
CREATE TABLE IF NOT EXISTS tx_events (
    id         BIGSERIAL PRIMARY KEY,
    tx_xid     xid8 NOT NULL DEFAULT pg_current_xact_id(),
    relayer_id VARCHAR(36) NOT NULL,
    tx_id      VARCHAR(255) NOT NULL,
    status     tx_status NOT NULL,
    tx_hash    BYTEA,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- A relayer's feed
CREATE INDEX IF NOT EXISTS tx_events_relayer_cursor ON tx_events (relayer_id, tx_xid, id);
