mod common;

use crate::common::prelude::*;

#[tokio::test]
async fn reorg() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    let anvil_port = anvil.port();

    let (_service, client) = ServiceBuilder::default()
        .hard_reorg_interval(Duration::from_secs(2))
        .build(&anvil, &db_url)
        .await?;

    let CreateApiKeyResponse { api_key } =
        client.create_relayer_api_key(DEFAULT_RELAYER_ID).await?;

    let provider = setup_provider(anvil.endpoint()).await?;

    // Send a transaction
    let value: U256 = parse_units("1", "ether")?.into();
    client
        .send_tx(
            &api_key,
            &SendTxRequest {
                to: ARBITRARY_ADDRESS.into(),
                value: value.into(),
                gas_limit: U256::from(21_000).into(),
                ..Default::default()
            },
        )
        .await?;

    await_balance(&provider, value).await?;

    // Drop anvil to simulate a reorg
    tracing::warn!("Dropping anvil & restarting at port {anvil_port}");
    drop(anvil);

    let anvil = AnvilBuilder::default().port(anvil_port).spawn().await?;
    let provider = setup_provider(anvil.endpoint()).await?;

    await_balance(&provider, value).await?;

    Ok(())
}

async fn await_balance(
    provider: &Provider<Http>,
    value: U256,
) -> eyre::Result<()> {
    for _ in 0..24 {
        let balance = match provider.get_balance(ARBITRARY_ADDRESS, None).await
        {
            Ok(balance) => balance,
            Err(err) => {
                tracing::warn!("Error getting balance: {:?}", err);
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
        };

        if balance == value {
            return Ok(());
        } else {
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }

    eyre::bail!("Balance not updated in time");
}
