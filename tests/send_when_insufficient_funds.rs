mod common;

use tx_sitter::client::ClientError;

use crate::common::prelude::*;

const ESCALATION_INTERVAL: Duration = Duration::from_secs(2);
const ANVIL_BLOCK_TIME: u64 = 6;

#[tokio::test]
async fn send_when_insufficient_funds() -> eyre::Result<()> {
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

    // Send a transaction
    let value: U256 = parse_units("1", "ether")?.into();
    for _ in 0..10 {
        let tx = client
            .send_tx(
                &api_key,
                &SendTxRequest {
                    to: ARBITRARY_ADDRESS.into(),
                    value: value.into(),
                    gas_limit: U256::from_dec_str("1000000000000")?.into(),
                    ..Default::default()
                },
            )
            .await;

        if let Err(ClientError::TxSitter(status_code, message)) = tx {
            assert_eq!(status_code, reqwest::StatusCode::UNPROCESSABLE_ENTITY);
            assert_eq!(
                message,
                "Relayer funds are insufficient for transaction to be mined."
            );
            return Ok(());
        }
    }

    eyre::bail!("Should return error response with information about insufficient funds.")
}
