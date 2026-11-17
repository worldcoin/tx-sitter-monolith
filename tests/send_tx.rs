mod common;

use tx_sitter_client::apis::relayer_v1_api::CreateTransactionParams;

use crate::common::prelude::*;

#[tokio::test]
async fn send_tx() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (_service, client) =
        ServiceBuilder::default().build(&anvil, &db_url).await?;
    let api_key = DEFAULT_API_KEY.to_string();

    let provider = setup_provider(anvil.endpoint()).await?;

    // Send a transaction
    let value: U256 = parse_units("1", "ether")?.into();
    let tx = tx_sitter_client::apis::relayer_v1_api::create_transaction(
        &client,
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

    await_balance(&provider, value, ARBITRARY_ADDRESS).await?;

    let tx_hash = await_mined_tx_hash(&client, &api_key, &tx.tx_id).await?;
    let receipt = provider.get_transaction_receipt(tx_hash.0).await?;

    assert!(
        receipt.is_some(),
        "No receipt for stored tx hash {tx_hash:?}"
    );

    Ok(())
}
