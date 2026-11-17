-- A relayer gives each nonce to at most one transaction. Applying this fails
-- if an earlier reset already gave a nonce to two transactions. Before
-- deploying, check that these return no rows:
--
-- duplicates that would fail the constraint
-- SELECT relayer_id, nonce, array_agg(id) FROM transactions GROUP BY 1,2 HAVING count(*) > 1;
-- relayers whose next create would hit it
-- SELECT r.id, r.nonce, max(t.nonce) FROM relayers r JOIN transactions t ON t.relayer_id = r.id GROUP BY 1,2 HAVING max(t.nonce) >= r.nonce;

-- Fail instead of queueing every query on the table behind a long lock wait
SET LOCAL lock_timeout = '5s';

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM   pg_constraint
        WHERE  conname = 'unique_relayer_nonce'
        AND    conrelid = 'transactions'::regclass
    ) THEN
        ALTER TABLE transactions
        ADD CONSTRAINT unique_relayer_nonce UNIQUE (relayer_id, nonce);
    END IF;
END
$$;
