SET LOCAL lock_timeout = '5s';

-- The L1 fees an OP-stack chain's GasPriceOracle reported, NULL on other
-- chains. Expand-only: binaries that predate L1 fee gating neither write nor
-- read it.
ALTER TABLE block_fees
ADD COLUMN IF NOT EXISTS l1_fees JSONB;
