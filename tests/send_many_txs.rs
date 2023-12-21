mod common;

use futures::stream::FuturesUnordered;
use futures::StreamExt;
use tx_sitter::server::routes::relayer::CreateApiKeyResponse;

use crate::common::prelude::*;

const ESCALATION_INTERVAL: Duration = Duration::from_secs(30);

#[tokio::test]
async fn send_many_txs() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = setup_anvil(DEFAULT_ANVIL_BLOCK_TIME).await?;

    let (_service, client) =
        setup_service(&anvil, &db_url, ESCALATION_INTERVAL).await?;

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
