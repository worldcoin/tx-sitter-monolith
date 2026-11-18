mod common;

use alloy::primitives::{address, Address, Bytes};
use alloy::providers::ext::AnvilApi;
use base_api_types::DecimalU256;
use hex_literal::hex;
use test_case::test_case;
use tx_sitter::config::OpStackConfig;
use tx_sitter::service::Service;
use tx_sitter_client::apis::relayer_v1_api::{
    CreateTransactionParams, GetTransactionParams,
};

use crate::common::prelude::*;

const GAS_PRICE_ORACLE: Address =
    address!("420000000000000000000000000000000000000F");

/// Answers every call with the storage slot that the call's selector names,
/// so that `anvil_setStorageAt` sets what `l1BaseFee()` and `blobBaseFee()`
/// return: `PUSH1 0 CALLDATALOAD PUSH1 0xe0 SHR SLOAD PUSH1 0 MSTORE PUSH1 0x20
/// PUSH1 0 RETURN`
const ORACLE_CODE: [u8; 15] = hex!("60003560e01c5460005260206000f3");

/// Reverts every call: `PUSH1 0 PUSH1 0 REVERT`
const REVERTING_ORACLE_CODE: [u8; 5] = hex!("60006000fd");

/// The selector of `l1BaseFee()`
const L1_BASE_FEE_SLOT: u64 = 0x519b4bd3;
/// The selector of `blobBaseFee()`
const BLOB_BASE_FEE_SLOT: u64 = 0xf8206140;

/// Worldchain's L1 fees on 2026-11-17
const L1_BASE_FEE: u64 = 168_039_652;
const BLOB_BASE_FEE: u64 = 10_274_004;

const MAX_L1_BASE_FEE: u64 = 200_000_000;
const MAX_BLOB_BASE_FEE: u64 = 20_000_000;

const FEE_ESTIMATION_INTERVAL: Duration = Duration::from_secs(1);
const MAX_FEE_AGE: Duration = Duration::from_secs(10);
const RPC_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a transaction must stay unsent to count as held, several
/// broadcast loops and fee estimations
const HOLD_DURATION: Duration = Duration::from_secs(8);

#[test_case(L1_BASE_FEE_SLOT, MAX_L1_BASE_FEE ; "l1 base fee")]
#[test_case(BLOB_BASE_FEE_SLOT, MAX_BLOB_BASE_FEE ; "blob base fee")]
#[tokio::test]
async fn holds_transactions_while_an_l1_fee_is_above_its_cap(
    fee_slot: u64,
    cap: u64,
) -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    let provider = setup_provider(anvil.endpoint()).await?;

    set_oracle_code(&provider, &ORACLE_CODE).await?;
    set_oracle_fee(&provider, L1_BASE_FEE_SLOT, L1_BASE_FEE).await?;
    set_oracle_fee(&provider, BLOB_BASE_FEE_SLOT, BLOB_BASE_FEE).await?;
    set_oracle_fee(&provider, fee_slot, cap + 1).await?;

    let (_service, client) = spawn_service(&anvil, &db_url, true).await?;

    let tx_id = send_tx(&client).await?;
    assert_held(&client, &tx_id).await?;

    set_oracle_fee(&provider, fee_slot, cap).await?;
    await_mined_tx_hash(&client, DEFAULT_API_KEY, &tx_id).await?;

    Ok(())
}

#[tokio::test]
async fn holds_transactions_once_the_l1_fees_are_unknown_for_max_fee_age(
) -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    let provider = setup_provider(anvil.endpoint()).await?;

    set_oracle_code(&provider, &ORACLE_CODE).await?;
    set_oracle_fee(&provider, L1_BASE_FEE_SLOT, L1_BASE_FEE).await?;
    set_oracle_fee(&provider, BLOB_BASE_FEE_SLOT, BLOB_BASE_FEE).await?;

    let (_service, client) = spawn_service(&anvil, &db_url, true).await?;

    set_oracle_code(&provider, &REVERTING_ORACLE_CODE).await?;

    let tx_id = send_tx(&client).await?;
    await_mined_tx_hash(&client, DEFAULT_API_KEY, &tx_id).await?;

    tokio::time::sleep(MAX_FEE_AGE).await;

    let tx_id = send_tx(&client).await?;
    assert_held(&client, &tx_id).await?;

    set_oracle_code(&provider, &ORACLE_CODE).await?;
    await_mined_tx_hash(&client, DEFAULT_API_KEY, &tx_id).await?;

    Ok(())
}

#[tokio::test]
async fn network_without_op_stack_ignores_the_oracle() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    let provider = setup_provider(anvil.endpoint()).await?;

    set_oracle_code(&provider, &REVERTING_ORACLE_CODE).await?;

    let (_service, client) = spawn_service(&anvil, &db_url, false).await?;

    let tx_id = send_tx(&client).await?;
    await_mined_tx_hash(&client, DEFAULT_API_KEY, &tx_id).await?;

    let l1_fees = connect_db(&db_url)
        .await?
        .get_l1_fees(DEFAULT_ANVIL_CHAIN_ID)
        .await?;
    assert_eq!(l1_fees, None);

    Ok(())
}

async fn set_oracle_code(
    provider: &RootProvider,
    code: &[u8],
) -> eyre::Result<()> {
    provider
        .anvil_set_code(GAS_PRICE_ORACLE, Bytes::copy_from_slice(code))
        .await?;

    Ok(())
}

async fn set_oracle_fee(
    provider: &RootProvider,
    slot: u64,
    fee: u64,
) -> eyre::Result<()> {
    provider
        .anvil_set_storage_at(
            GAS_PRICE_ORACLE,
            U256::from(slot),
            B256::from(U256::from(fee)),
        )
        .await?;

    Ok(())
}

/// Starts a service with the anvil network, as an OP-stack network capped at
/// [`MAX_L1_BASE_FEE`] and [`MAX_BLOB_BASE_FEE`] if `is_op_stack`, and the
/// default relayer, and waits for its first fee estimates.
async fn spawn_service(
    anvil: &alloy::node_bindings::AnvilInstance,
    db_url: &str,
    is_op_stack: bool,
) -> eyre::Result<(Service, Configuration)> {
    let op_stack = is_op_stack.then(|| OpStackConfig {
        max_l1_base_fee: DecimalU256(U256::from(MAX_L1_BASE_FEE)),
        max_blob_base_fee: DecimalU256(U256::from(MAX_BLOB_BASE_FEE)),
        max_fee_age: MAX_FEE_AGE,
    });

    let (service, client) = ServiceBuilder::default()
        .without_defaults()
        .fee_estimation_interval(FEE_ESTIMATION_INTERVAL)
        .rpc_timeout(RPC_TIMEOUT)
        .network(NetworkConfig {
            op_stack,
            ..anvil_network(anvil)
        })
        .relayer(default_relayer())
        .build(anvil, db_url)
        .await?;

    await_estimates(&service, DEFAULT_ANVIL_CHAIN_ID, Duration::from_secs(30))
        .await?;

    Ok((service, client))
}

async fn send_tx(client: &Configuration) -> eyre::Result<String> {
    let tx = tx_sitter_client::apis::relayer_v1_api::create_transaction(
        client,
        CreateTransactionParams {
            api_token: DEFAULT_API_KEY.to_string(),
            send_tx_request: SendTxRequest {
                to: ARBITRARY_ADDRESS.into(),
                value: U256::from(1).into(),
                gas_limit: U256::from(21_000).into(),
                ..Default::default()
            },
        },
    )
    .await?;

    Ok(tx.tx_id)
}

/// Fails if the transaction is broadcast within [`HOLD_DURATION`].
async fn assert_held(client: &Configuration, tx_id: &str) -> eyre::Result<()> {
    let deadline = tokio::time::Instant::now() + HOLD_DURATION;

    while tokio::time::Instant::now() < deadline {
        let tx = tx_sitter_client::apis::relayer_v1_api::get_transaction(
            client,
            GetTransactionParams {
                api_token: DEFAULT_API_KEY.to_string(),
                tx_id: tx_id.to_string(),
            },
        )
        .await?;

        if let Some(tx_hash) = tx.tx_hash {
            eyre::bail!("Transaction {tx_id} was broadcast as {tx_hash:?}");
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    Ok(())
}
