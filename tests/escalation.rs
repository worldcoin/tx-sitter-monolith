mod common;

use tx_sitter_client::apis::admin_v1_api::RelayerCreateApiKeyParams;
use tx_sitter_client::apis::configuration::Configuration;
use tx_sitter_client::apis::relayer_v1_api::{
    CreateTransactionParams, GetTransactionParams,
};

use crate::common::prelude::*;

const ESCALATION_INTERVAL: Duration = Duration::from_secs(2);
const ANVIL_BLOCK_TIME: u64 = 10;

#[tokio::test]
async fn escalation() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default()
        .block_time(ANVIL_BLOCK_TIME)
        .spawn()
        .await?;

    let (_service, client) = ServiceBuilder::default()
        .escalation_interval(ESCALATION_INTERVAL)
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
    let provider = setup_provider(anvil.endpoint()).await?;

    // Send a transaction
    let value: U256 = parse_units("1", "ether")?.into();
    let tx = tx_sitter_client::apis::relayer_v1_api::create_transaction(
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

    let initial_tx_hash = get_tx_hash(&client, &api_key, &tx.tx_id).await?;

    await_balance(&provider, value).await?;
    let final_tx_hash = get_tx_hash(&client, &api_key, &tx.tx_id).await?;

    assert_ne!(
        initial_tx_hash, final_tx_hash,
        "Escalation should have occurred"
    );

    Ok(())
}

async fn await_balance(
    provider: &Provider<Http>,
    value: U256,
) -> eyre::Result<()> {
    for _ in 0..24 {
        let balance = provider.get_balance(ARBITRARY_ADDRESS, None).await?;

        if balance == value {
            return Ok(());
        } else {
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }

    eyre::bail!("Balance not updated in time");
}

async fn get_tx_hash(
    client: &Configuration,
    api_key: &str,
    tx_id: &str,
) -> eyre::Result<H256> {
    loop {
        let tx = tx_sitter_client::apis::relayer_v1_api::get_transaction(
            client,
            GetTransactionParams {
                api_token: api_key.to_owned(),
                tx_id: tx_id.to_owned(),
            },
        )
        .await?;

        if let Some(tx_hash) = tx.tx_hash {
            return Ok(tx_hash.0);
        } else {
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }
}
