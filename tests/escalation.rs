mod common;

use crate::common::prelude::*;

const ESCALATION_INTERVAL: Duration = Duration::from_secs(2);
const ANVIL_BLOCK_TIME: u64 = 6;

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
        client.create_relayer_api_key(DEFAULT_RELAYER_ID).await?;

    let provider = setup_provider(anvil.endpoint()).await?;

    // Send a transaction
    let value: U256 = parse_units("1", "ether")?.into();
    let tx = client
        .send_tx(
            &api_key,
            &SendTxRequest {
                to: ARBITRARY_ADDRESS,
                value,
                gas_limit: U256::from(21_000),
                ..Default::default()
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
    client: &TxSitterClient,
    api_key: &ApiKey,
    tx_id: &str,
) -> eyre::Result<H256> {
    loop {
        let tx = client.get_tx(api_key, tx_id).await?;

        if let Some(tx_hash) = tx.tx_hash {
            return Ok(tx_hash);
        } else {
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }
}
