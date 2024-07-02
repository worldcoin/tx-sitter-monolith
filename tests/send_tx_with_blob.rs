mod common;

use alloy::primitives::Address;
use alloy::providers::{Provider, ProviderBuilder};

use tokio::time::Duration;
use tx_sitter::types::TransactionPriority;

use crate::common::prelude::*;

#[tokio::test]
async fn send_tx_with_blob() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (_service, client) =
        ServiceBuilder::default().build(&anvil, &db_url).await?;
    let CreateApiKeyResponse { api_key } =
        client.create_relayer_api_key(DEFAULT_RELAYER_ID).await?;

    let endpoint = anvil.endpoint().parse()?;
    let provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .on_http(endpoint);

    // Send a transaction
    let ethers_value: U256 = parse_units("1", "ether")?.into();
    let mut value = [0_u8; 32];
    ethers_value.to_little_endian(&mut value);
    let tx_value = alloy::primitives::U256::from_le_slice(&value);

    let req = SendTxRequest {
        to: ARBITRARY_ADDRESS.into(),
        value: ethers_value.into(),
        gas_limit: U256::from(21_000).into(),
        blobs: Some(vec![vec![1_u8; 10]]),
        data: None,
        priority: TransactionPriority::Regular,
        tx_id: None,
    };

    let json_string = serde_json::to_string(&req).unwrap();

    tracing::info!("==== Sending transaction: {:?}", json_string);

    client.send_tx(&api_key, &req).await?;

    let address = Address::from_slice(&ARBITRARY_ADDRESS.to_fixed_bytes());

    for _ in 0..10 {
        let balance = provider.get_balance(address).await?;

        if balance == tx_value {
            return Ok(());
        } else {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    panic!("Transaction was not sent")
}
