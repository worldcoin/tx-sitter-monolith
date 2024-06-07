-- Step 1: Identify and remove duplicate rpcs
WITH ranked_rpcs AS (
    SELECT
        id,
        chain_id,
        kind,
        ROW_NUMBER() OVER (PARTITION BY chain_id, kind ORDER BY id DESC) AS rank
    FROM
        rpcs
)
DELETE FROM rpcs
WHERE id IN (
    SELECT id
    FROM ranked_rpcs
    WHERE rank > 1
);

-- Step 2: Add the uniqueness constraint
ALTER TABLE rpcs
ADD CONSTRAINT unique_chain_id_kind UNIQUE (chain_id, kind);
