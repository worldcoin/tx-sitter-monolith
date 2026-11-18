mod common;

use tx_sitter_client::apis::relayer_v1_api::CreateTransactionParams;

use crate::common::prelude::*;

#[tokio::test]
async fn send_many_txs() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (_service, client) =
        ServiceBuilder::default().build(&anvil, &db_url).await?;

    let api_key = DEFAULT_API_KEY.to_string();

    let provider = setup_provider(anvil.endpoint()).await?;

    // Send a transaction
    let value: U256 = parse_units("10", "ether")?.into();

    let num_transfers = 10;

    let mut tasks = FuturesUnordered::new();
    for _ in 0..num_transfers {
        let client = &client;
        tasks.push(async {
            tx_sitter_client::apis::relayer_v1_api::create_transaction(
                client,
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

            Ok(())
        });
    }

    while let Some(result) = tasks.next().await {
        let result: eyre::Result<()> = result;
        result?;
    }

    let expected_balance = value * U256::from(num_transfers);
    await_balance(&provider, expected_balance, ARBITRARY_ADDRESS).await?;

    Ok(())
}
