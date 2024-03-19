ALTER TABLE relayers
RENAME COLUMN gas_limits TO gas_price_limits;

ALTER TABLE relayers
ADD COLUMN enabled BOOL NOT NULL DEFAULT TRUE;
