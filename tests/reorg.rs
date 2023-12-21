mod common;

use ethers::prelude::{Http, Provider};
use ethers::types::H256;
use tx_sitter::api_key::ApiKey;
use tx_sitter::client::TxSitterClient;
use tx_sitter::server::routes::relayer::CreateApiKeyResponse;

use crate::common::prelude::*;

const ESCALATION_INTERVAL: Duration = Duration::from_secs(2);

#[tokio::test]
async fn reorg() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default()
        .spawn()
        .await?;
    let anvil_port = anvil.port();

    let (_service, client) =
        setup_service(&anvil, &db_url, ESCALATION_INTERVAL).await?;

    let CreateApiKeyResponse { api_key } =
        client.create_relayer_api_key(DEFAULT_RELAYER_ID).await?;

    let provider = setup_provider(anvil.endpoint()).await?;

    // Send a transaction
    let value: U256 = parse_units("1", "ether")?.into();
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

    await_balance(&provider, value).await?;

    // Drop anvil to simulate a reorg
    drop(anvil);

    AnvilBuilder::default().port(anvil_port).spawn().await?;
    await_balance(&provider, value).await?;

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
