mod common;

use tx_sitter::db::Database;
use tx_sitter_client::apis::admin_v1_api::{
    CreateNetworkParams, RelayerCreateApiKeyParams,
};
use tx_sitter_client::apis::relayer_v1_api::{
    CreateTransactionParams, GetTransactionParams,
};

use crate::common::prelude::*;

/// A service that runs tasks indexes a block, estimates its fees and
/// broadcasts pending txs within a few blocks
const TASK_GRACE_PERIOD: Duration =
    Duration::from_secs(3 * DEFAULT_ANVIL_BLOCK_TIME);

#[tokio::test]
async fn api_role_runs_no_background_tasks() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    let db = connect_db(&db_url).await?;

    let (_service, client) = ServiceBuilder::default()
        .role(ServiceRole::Api)
        .build(&anvil, &db_url)
        .await?;
    let CreateApiKeyResponse { api_key } =
        tx_sitter_client::apis::admin_v1_api::relayer_create_api_key(
            &client,
            RelayerCreateApiKeyParams {
                relayer_id: DEFAULT_RELAYER_ID.to_string(),
            },
        )
        .await?;

    let value: U256 = parse_units("1", "ether")?.into();
    let SendTxResponse { tx_id } =
        tx_sitter_client::apis::relayer_v1_api::create_transaction(
            &client,
            CreateTransactionParams {
                api_token: api_key.clone(),
                send_tx_request: SendTxRequest {
                    to: ARBITRARY_ADDRESS.into(),
                    value: value.into(),
                    gas_limit: U256::from(21_000).into(),
                    ..Default::default()
                },
            },
        )
        .await?;

    tokio::time::sleep(TASK_GRACE_PERIOD).await;
    assert_chain_not_indexed(&db).await?;

    tx_sitter_client::apis::admin_v1_api::create_network(
        &client,
        CreateNetworkParams {
            chain_id: DEFAULT_ANVIL_CHAIN_ID as i32,
            create_network_request: CreateNetworkRequest::new(
                "Anvil".to_string(),
                anvil.endpoint(),
                anvil.ws_endpoint(),
            ),
        },
    )
    .await?;

    tokio::time::sleep(TASK_GRACE_PERIOD).await;
    assert_chain_not_indexed(&db).await?;

    let tx = tx_sitter_client::apis::relayer_v1_api::get_transaction(
        &client,
        GetTransactionParams {
            api_token: api_key,
            tx_id,
        },
    )
    .await?;
    assert_eq!(tx.tx_hash, None);
    assert_eq!(tx.status, None);

    let provider = setup_provider(anvil.endpoint()).await?;
    let balance = provider.get_balance(ARBITRARY_ADDRESS).await?;
    assert_eq!(balance, U256::ZERO);

    Ok(())
}

async fn assert_chain_not_indexed(db: &Database) -> eyre::Result<()> {
    let latest_block =
        db.get_latest_block_number(DEFAULT_ANVIL_CHAIN_ID).await?;
    assert_eq!(latest_block, None, "blocks were indexed");

    let latest_fees = db
        .get_latest_block_fees_by_chain_id(DEFAULT_ANVIL_CHAIN_ID)
        .await?;
    assert!(latest_fees.is_none(), "fees were estimated");

    Ok(())
}
