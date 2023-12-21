mod common;

use tx_sitter::server::routes::relayer::CreateApiKeyResponse;

use crate::common::prelude::*;

const ESCALATION_INTERVAL: Duration = Duration::from_secs(2);
const ANVIL_BLOCK_TIME: u64 = 12;

#[tokio::test]
async fn escalation() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = setup_anvil(ANVIL_BLOCK_TIME).await?;

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

    for _ in 0..24 {
        let balance = provider.get_balance(ARBITRARY_ADDRESS, None).await?;

        if balance == value {
            return Ok(());
        } else {
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }

    panic!("Transaction was not sent")
}
