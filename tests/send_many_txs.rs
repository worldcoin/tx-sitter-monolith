mod common;

use crate::common::prelude::*;

#[tokio::test]
async fn send_many_txs() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (_service, client) =
        ServiceBuilder::default().build(&anvil, &db_url).await?;

    let CreateApiKeyResponse { api_key } =
        client.create_relayer_api_key(DEFAULT_RELAYER_ID).await?;

    let provider = setup_provider(anvil.endpoint()).await?;

    // Send a transaction
    let value: U256 = parse_units("10", "ether")?.into();
    let num_transfers = 10;

    let mut tasks = FuturesUnordered::new();
    for _ in 0..num_transfers {
        let client = &client;
        tasks.push(async {
            client
                .send_tx(
                    &api_key,
                    &SendTxRequest {
                        to: ARBITRARY_ADDRESS,
                        value: value.into(),
                        gas_limit: U256::from(21_000).into(),
                        ..Default::default()
                    },
                )
                .await?;

            Ok(())
        });
    }

    while let Some(result) = tasks.next().await {
        let result: eyre::Result<()> = result;
        result?;
    }

    let expected_balance = value * num_transfers;
    await_balance(&provider, expected_balance, ARBITRARY_ADDRESS).await?;

    Ok(())
}
