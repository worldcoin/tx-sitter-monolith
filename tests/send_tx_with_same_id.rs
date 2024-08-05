mod common;

use reqwest::StatusCode;
use tx_sitter::client::ClientError;

use crate::common::prelude::*;

#[tokio::test]
async fn send_tx_with_same_id() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (_service, client) =
        ServiceBuilder::default().build(&anvil, &db_url).await?;
    let CreateApiKeyResponse { api_key } =
        client.create_relayer_api_key(DEFAULT_RELAYER_ID).await?;

    let tx_id = Some("tx-1".to_string());

    // Send a transaction
    let value: U256 = parse_units("1", "ether")?.into();
    client
        .send_tx(
            &api_key,
            &SendTxRequest {
                to: ARBITRARY_ADDRESS.into(),
                value: value.into(),
                gas_limit: U256::from(21_000).into(),
                tx_id: tx_id.clone(),
                ..Default::default()
            },
        )
        .await?;

    let res = client
        .send_tx(
            &api_key,
            &SendTxRequest {
                to: ARBITRARY_ADDRESS.into(),
                value: value.into(),
                gas_limit: U256::from(21_000).into(),
                tx_id: tx_id.clone(),
                ..Default::default()
            },
        )
        .await;

    if let ClientError::TxSitter(status_code, message) = res.unwrap_err() {
        assert_eq!(status_code, StatusCode::CONFLICT);
        assert_eq!(message, "Transaction with same id already exists.");

        return Ok(());
    }

    panic!("Should return error on second insert with same id.")
}
