SET LOCAL lock_timeout = '5s';

-- When the worker synced the relayer's nonce from its chain, since the
-- relayer was last enabled or moved to another account. The API allocates
-- nonces only to synced relayers, so that a key with on-chain history, such
-- as one imported from tx-sitter-aws, doesn't start at nonce 0.
-- Expand-only: binaries that predate it neither read nor write it.
ALTER TABLE relayers ADD COLUMN IF NOT EXISTS nonce_synced_at TIMESTAMPTZ;

-- Binaries that predate it synced every relayer's nonce with every block, so
-- every relayer that exists now is synced. Relayers the config adds later
-- start unsynced.
UPDATE relayers
SET    nonce_synced_at = now()
WHERE  nonce_synced_at IS NULL;
