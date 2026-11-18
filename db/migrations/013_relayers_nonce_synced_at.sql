SET LOCAL lock_timeout = '5s';

-- When the worker first synced the relayer's nonce from its chain. The API
-- allocates nonces only to synced relayers, so that a key with on-chain
-- history, such as one imported from tx-sitter-aws, doesn't start at nonce 0.
-- Expand-only: binaries that predate it neither read nor write it.
ALTER TABLE relayers ADD COLUMN IF NOT EXISTS nonce_synced_at TIMESTAMPTZ;

-- Relayers that synced or allocated nonces before keep accepting
-- transactions. A nonce below the chain's is already used, so it moves up.
UPDATE relayers
SET    nonce_synced_at = now(),
       nonce = GREATEST(nonce, current_nonce)
WHERE  nonce_synced_at IS NULL
AND    (
           current_nonce > 0
        OR nonce > 0
        OR EXISTS (SELECT 1 FROM transactions t WHERE t.relayer_id = relayers.id)
       );
