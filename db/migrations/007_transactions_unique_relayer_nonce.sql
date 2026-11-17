-- A relayer gives each nonce to at most one transaction. Applying this fails
-- if an earlier reset already gave a nonce to two transactions.
ALTER TABLE transactions
ADD CONSTRAINT unique_relayer_nonce UNIQUE (relayer_id, nonce);
