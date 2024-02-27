mod common;

use crate::common::prelude::*;

#[tokio::test]
async fn send_too_many_txs() -> eyre::Result<()> {
    setup_tracing();

    panic!("UNIMPLEMENTED!");

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
                        value,
                        gas_limit: U256::from(21_000),
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
    for _ in 0..50 {
        let balance = provider.get_balance(ARBITRARY_ADDRESS, None).await?;

        tracing::info!(?balance, ?expected_balance, "Checking balance");

        if balance == expected_balance {
            return Ok(());
        } else {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    panic!("Transactions were not sent")
}
