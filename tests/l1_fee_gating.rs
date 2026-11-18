mod common;

use std::sync::OnceLock;

use alloy::node_bindings::AnvilInstance;
use alloy::primitives::{address, Address, Bytes};
use alloy::providers::ext::AnvilApi;
use base_api_types::DecimalU256;
use eyre::ContextCompat;
use hex_literal::hex;
use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use test_case::test_case;
use tx_sitter::broadcast_utils::gas_estimation::FeesEstimate;
use tx_sitter::config::OpStackConfig;
use tx_sitter::db::Database;
use tx_sitter::service::Service;
use tx_sitter_client::apis::relayer_v1_api::GetTransactionParams;

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
const ESCALATION_INTERVAL: Duration = Duration::from_secs(2);

/// How long the gate takes to follow a change of the oracle's fees, a few
/// reads
const FEE_CHANGE_DELAY: Duration = Duration::from_secs(3);

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

    install_oracle(&provider).await?;
    set_oracle_fee(&provider, fee_slot, cap + 1).await?;

    let (_service, client) = spawn_service(
        service_builder(op_stack_network(&anvil)),
        &anvil,
        &db_url,
    )
    .await?;

    let tx_id = send_transfer(&client, DEFAULT_API_KEY, U256::from(1)).await?;
    assert_held(&client, &tx_id).await?;

    set_oracle_fee(&provider, fee_slot, cap).await?;
    await_mined_tx_hash(&client, DEFAULT_API_KEY, &tx_id).await?;

    Ok(())
}

#[tokio::test]
async fn holds_escalations_while_an_l1_fee_is_above_its_cap() -> eyre::Result<()>
{
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    let provider = setup_provider(anvil.endpoint()).await?;
    let db = connect_db(&db_url).await?;

    install_oracle(&provider).await?;

    let (_service, client) = spawn_service(
        service_builder(op_stack_network(&anvil))
            .escalation_interval(ESCALATION_INTERVAL),
        &anvil,
        &db_url,
    )
    .await?;

    // Without new blocks the transaction stays in flight and is escalated
    provider.anvil_set_interval_mining(0).await?;
    let tx_id = send_transfer(&client, DEFAULT_API_KEY, U256::from(1)).await?;
    await_escalation_count(&db, &tx_id, 1).await?;

    set_oracle_fee(&provider, L1_BASE_FEE_SLOT, MAX_L1_BASE_FEE + 1).await?;
    tokio::time::sleep(FEE_CHANGE_DELAY).await;
    let held_escalation_count = escalation_count(&db, &tx_id)
        .await?
        .context("Missing sent transaction")?;

    tokio::time::sleep(ESCALATION_INTERVAL * 3).await;
    assert_eq!(
        escalation_count(&db, &tx_id).await?,
        Some(held_escalation_count),
        "The transaction was escalated while an L1 fee was above its cap"
    );

    set_oracle_fee(&provider, L1_BASE_FEE_SLOT, MAX_L1_BASE_FEE).await?;
    await_escalation_count(&db, &tx_id, held_escalation_count + 1).await?;

    provider
        .anvil_set_interval_mining(DEFAULT_ANVIL_BLOCK_TIME)
        .await?;
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

    install_oracle(&provider).await?;

    let (_service, client) = spawn_service(
        service_builder(op_stack_network(&anvil)),
        &anvil,
        &db_url,
    )
    .await?;

    set_oracle_code(&provider, &REVERTING_ORACLE_CODE).await?;

    let tx_id = send_transfer(&client, DEFAULT_API_KEY, U256::from(1)).await?;
    await_mined_tx_hash(&client, DEFAULT_API_KEY, &tx_id).await?;

    tokio::time::sleep(MAX_FEE_AGE).await;

    let tx_id = send_transfer(&client, DEFAULT_API_KEY, U256::from(1)).await?;
    assert_held(&client, &tx_id).await?;

    set_oracle_code(&provider, &ORACLE_CODE).await?;
    await_mined_tx_hash(&client, DEFAULT_API_KEY, &tx_id).await?;

    Ok(())
}

#[tokio::test]
async fn holds_transactions_until_the_l1_fees_are_first_read(
) -> eyre::Result<()> {
    setup_tracing();
    let read_failures = l1_fee_read_failures();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    let provider = setup_provider(anvil.endpoint()).await?;

    set_oracle_code(&provider, &REVERTING_ORACLE_CODE).await?;

    let (_service, client) = spawn_service(
        service_builder(op_stack_network(&anvil)),
        &anvil,
        &db_url,
    )
    .await?;
    await_l1_fee_read_failure(read_failures).await?;

    let tx_id = send_transfer(&client, DEFAULT_API_KEY, U256::from(1)).await?;
    assert_held(&client, &tx_id).await?;

    install_oracle(&provider).await?;
    await_mined_tx_hash(&client, DEFAULT_API_KEY, &tx_id).await?;

    Ok(())
}

/// The chain halts, or its WS RPC goes down, while its HTTP RPCs still
/// answer: the L1 fees are still read, so transactions still go out.
#[tokio::test]
async fn reads_the_l1_fees_while_indexing_stalls() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    let provider = setup_provider(anvil.endpoint()).await?;
    let unresponsive_rpc = UnresponsiveRpc::spawn().await?;

    install_oracle(&provider).await?;

    let (_service, client) = service_builder(NetworkConfig {
        ws_rpc: unresponsive_rpc.network(DEFAULT_ANVIL_CHAIN_ID).ws_rpc,
        ..op_stack_network(&anvil)
    })
    .build(&anvil, &db_url)
    .await?;
    save_block_fees(&connect_db(&db_url).await?).await?;

    tokio::time::sleep(MAX_FEE_AGE + FEE_ESTIMATION_INTERVAL).await;

    let tx_id = send_transfer(&client, DEFAULT_API_KEY, U256::from(1)).await?;
    await_broadcast(&client, &provider, &tx_id).await?;

    Ok(())
}

#[tokio::test]
async fn network_without_op_stack_ignores_the_oracle() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    let provider = setup_provider(anvil.endpoint()).await?;

    set_oracle_code(&provider, &REVERTING_ORACLE_CODE).await?;

    let (_service, client) =
        spawn_service(service_builder(anvil_network(&anvil)), &anvil, &db_url)
            .await?;

    let tx_id = send_transfer(&client, DEFAULT_API_KEY, U256::from(1)).await?;
    await_mined_tx_hash(&client, DEFAULT_API_KEY, &tx_id).await?;

    let l1_fees = connect_db(&db_url)
        .await?
        .get_l1_fees(DEFAULT_ANVIL_CHAIN_ID)
        .await?;
    assert_eq!(l1_fees, None);

    Ok(())
}

/// Puts an oracle at the predeploy that reports Worldchain's L1 fees.
async fn install_oracle(provider: &RootProvider) -> eyre::Result<()> {
    set_oracle_code(provider, &ORACLE_CODE).await?;
    set_oracle_fee(provider, L1_BASE_FEE_SLOT, L1_BASE_FEE).await?;
    set_oracle_fee(provider, BLOB_BASE_FEE_SLOT, BLOB_BASE_FEE).await?;

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

/// The anvil network, as an OP-stack network capped at [`MAX_L1_BASE_FEE`]
/// and [`MAX_BLOB_BASE_FEE`]
fn op_stack_network(anvil: &AnvilInstance) -> NetworkConfig {
    NetworkConfig {
        op_stack: Some(OpStackConfig {
            max_l1_base_fee: DecimalU256(U256::from(MAX_L1_BASE_FEE)),
            max_blob_base_fee: DecimalU256(U256::from(MAX_BLOB_BASE_FEE)),
            max_fee_age: MAX_FEE_AGE,
        }),
        ..anvil_network(anvil)
    }
}

/// A service with `network` and the default relayer, which reads the fees
/// every [`FEE_ESTIMATION_INTERVAL`]
fn service_builder(network: NetworkConfig) -> ServiceBuilder {
    ServiceBuilder::default()
        .without_defaults()
        .fee_estimation_interval(FEE_ESTIMATION_INTERVAL)
        .rpc_timeout(RPC_TIMEOUT)
        .network(network)
        .relayer(default_relayer())
}

/// Starts the service of `builder` and waits for its first fee estimates of
/// the anvil network.
async fn spawn_service(
    builder: ServiceBuilder,
    anvil: &AnvilInstance,
    db_url: &str,
) -> eyre::Result<(Service, Configuration)> {
    let (service, client) = builder.build(anvil, db_url).await?;

    await_estimates(&service, DEFAULT_ANVIL_CHAIN_ID, Duration::from_secs(30))
        .await?;

    Ok((service, client))
}

/// Saves a block of the anvil network with fee estimates, as indexing would
/// have before it stalled.
async fn save_block_fees(db: &Database) -> eyre::Result<()> {
    let gwei: U256 = parse_units("1", "gwei")?.into();
    db.save_block(1, DEFAULT_ANVIL_CHAIN_ID, chrono::Utc::now(), &[])
        .await?;
    db.save_block_fees(
        1,
        DEFAULT_ANVIL_CHAIN_ID,
        &FeesEstimate {
            base_fee_per_gas: gwei * U256::from(10),
            percentile_fees: vec![gwei; 5],
        },
        gwei,
    )
    .await?;

    Ok(())
}

/// Fails if the transaction is broadcast within [`HOLD_DURATION`].
async fn assert_held(client: &Configuration, tx_id: &str) -> eyre::Result<()> {
    let deadline = tokio::time::Instant::now() + HOLD_DURATION;

    while tokio::time::Instant::now() < deadline {
        if let Some(tx_hash) = tx_hash(client, tx_id).await? {
            eyre::bail!("Transaction {tx_id} was broadcast as {tx_hash:?}");
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    Ok(())
}

/// Waits until the transaction is broadcast and anvil knows it.
async fn await_broadcast(
    client: &Configuration,
    provider: &RootProvider,
    tx_id: &str,
) -> eyre::Result<()> {
    for _ in 0..30 {
        if let Some(tx_hash) = tx_hash(client, tx_id).await? {
            if provider.get_transaction_by_hash(tx_hash).await?.is_some() {
                return Ok(());
            }
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    eyre::bail!("Transaction {tx_id} was not broadcast in time");
}

async fn tx_hash(
    client: &Configuration,
    tx_id: &str,
) -> eyre::Result<Option<B256>> {
    let tx = tx_sitter_client::apis::relayer_v1_api::get_transaction(
        client,
        GetTransactionParams {
            api_token: DEFAULT_API_KEY.to_string(),
            tx_id: tx_id.to_string(),
        },
    )
    .await?;

    Ok(tx.tx_hash.map(|tx_hash| tx_hash.0))
}

/// Returns how often the transaction was escalated, `None` until it is
/// broadcast.
async fn escalation_count(
    db: &Database,
    tx_id: &str,
) -> eyre::Result<Option<i64>> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT escalation_count FROM sent_transactions WHERE tx_id = $1",
    )
    .bind(tx_id)
    .fetch_optional(&db.pool)
    .await?;

    Ok(row.map(|(escalation_count,)| escalation_count))
}

async fn await_escalation_count(
    db: &Database,
    tx_id: &str,
    expected: i64,
) -> eyre::Result<()> {
    for _ in 0..30 {
        if escalation_count(db, tx_id).await? >= Some(expected) {
            return Ok(());
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    eyre::bail!("Transaction {tx_id} was not escalated {expected} times");
}

/// Returns how often reading the L1 fees failed in this process, across
/// every chain.
///
/// The first call installs the recorder, so call it before the service whose
/// failures it counts starts.
fn l1_fee_read_failures() -> u64 {
    static METRICS: OnceLock<Snapshotter> = OnceLock::new();
    let metrics = METRICS.get_or_init(|| {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        recorder
            .install()
            .expect("No other metrics recorder is installed");

        snapshotter
    });

    metrics
        .snapshot()
        .into_vec()
        .into_iter()
        .filter_map(|(key, _, _, value)| match value {
            DebugValue::Counter(count)
                if key.key().name() == "l1_fee_read_failures" =>
            {
                Some(count)
            }
            _ => None,
        })
        .sum()
}

async fn await_l1_fee_read_failure(failures_before: u64) -> eyre::Result<()> {
    for _ in 0..30 {
        if l1_fee_read_failures() > failures_before {
            return Ok(());
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    eyre::bail!("No L1 fee read failed");
}
