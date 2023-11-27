CREATE TYPE tx_status AS ENUM ('pending', 'mined', 'finalized');

CREATE TYPE rpc_kind AS ENUM ('http', 'ws');

create table networks (
    chain_id BIGINT PRIMARY KEY,
    name VARCHAR(255) NOT NULL
);

create table rpcs (
    id BIGSERIAL PRIMARY KEY,
    chain_id BIGINT NOT NULL REFERENCES networks(chain_id),
    url VARCHAR(255) NOT NULL,
    kind rpc_kind NOT NULL
);

CREATE TABLE relayers (
    id VARCHAR(255) PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    chain_id BIGINT NOT NULL,
    key_id VARCHAR(255) NOT NULL,
    address BYTEA NOT NULL,
    -- The local nonce value
    nonce BIGINT NOT NULL,
    -- The confirmed nonce value
    current_nonce BIGINT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- CREATE TABLE relayer_deps (
--     relayer_id VARCHAR(255) NOT NULL REFERENCES relayers(id),
-- );

-- Constant tx data - once a tx is created, this data should never change
CREATE TABLE transactions (
    id VARCHAR(255) PRIMARY KEY,
    tx_to BYTEA NOT NULL,
    data BYTEA NOT NULL,
    value BYTEA NOT NULL,
    gas_limit BYTEA NOT NULL,
    nonce BIGINT NOT NULL,
    relayer_id VARCHAR(255) NOT NULL REFERENCES relayers(id)
);

-- Sent transaction attempts
CREATE TABLE tx_hashes (
    tx_hash BYTEA PRIMARY KEY,
    tx_id VARCHAR(255) NOT NULL REFERENCES transactions(id) ON DELETE CASCADE,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    max_fee_per_gas BYTEA NOT NULL,
    max_priority_fee_per_gas BYTEA NOT NULL,
    escalated BOOL NOT NULL DEFAULT FALSE
);

-- Dynamic tx data & data used for escalations
CREATE TABLE sent_transactions (
    tx_id                            VARCHAR(255) PRIMARY KEY REFERENCES transactions(id) ON DELETE CASCADE,
    initial_max_fee_per_gas          BYTEA        NOT NULL,
    initial_max_priority_fee_per_gas BYTEA        NOT NULL,
    created_at                       TIMESTAMP    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                       TIMESTAMP    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    first_submitted_at               TIMESTAMP    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    mined_at                         TIMESTAMP,
    escalation_count                 BIGINT       NOT NULL DEFAULT 0,
    last_escalation                  TIMESTAMP    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    reorg                            BOOL         NOT NULL DEFAULT FALSE,
    status                           tx_status    NOT NULL DEFAULT 'pending',
    -- If the status is mined or finalized this should be the actual tx hash that is mined or finalized
    valid_tx_hash                    BYTEA        NOT NULL REFERENCES tx_hashes(tx_hash)
);

CREATE TABLE blocks (
    block_number BIGINT NOT NULL,
    chain_id BIGINT NOT NULL,
    timestamp TIMESTAMPTZ NOT NULL,
    fee_estimate JSON,
    PRIMARY KEY (block_number, chain_id)
);

CREATE TABLE block_txs (
    block_number BIGINT NOT NULL,
    chain_id BIGINT NOT NULL,
    tx_hash BYTEA NOT NULL,
    PRIMARY KEY (block_number, chain_id, tx_hash),
    FOREIGN KEY (block_number, chain_id) REFERENCES blocks (block_number, chain_id) ON DELETE CASCADE,
    FOREIGN KEY (tx_hash) REFERENCES tx_hashes (tx_hash)
);