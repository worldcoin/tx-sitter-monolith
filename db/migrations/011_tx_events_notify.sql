-- Fail instead of queueing every query on the table behind a long lock wait
SET LOCAL lock_timeout = '5s';

-- Wakes the feed's waiting requests with a notification per relayer on the
-- tx_events channel, which Postgres delivers when the writing transaction
-- commits
CREATE OR REPLACE FUNCTION notify_tx_events() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_notify('tx_events', relayer_id)
    FROM    (SELECT DISTINCT relayer_id FROM new_events) AS relayers;

    RETURN NULL;
END;
$$;

DROP TRIGGER IF EXISTS tx_events_notify ON tx_events;
CREATE TRIGGER tx_events_notify
AFTER INSERT ON tx_events
REFERENCING NEW TABLE AS new_events
FOR EACH STATEMENT EXECUTE FUNCTION notify_tx_events();
