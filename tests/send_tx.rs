mod common;

use service::server::routes::relayer::CreateApiKeyResponse;

use crate::common::prelude::*;

const ESCALATION_INTERVAL: Duration = Duration::from_secs(30);

#[tokio::test]
async fn send_tx() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let double_anvil = setup_double_anvil().await?;

    let (_service, client) =
        setup_service(&double_anvil, &db_url, ESCALATION_INTERVAL).await?;

    let CreateRelayerResponse {
        address: relayer_address,
        relayer_id,
    } = client
        .create_relayer(&CreateRelayerRequest {
            name: "Test relayer".to_string(),
            chain_id: DEFAULT_ANVIL_CHAIN_ID,
        })
        .await?;

    let CreateApiKeyResponse { api_key } =
        client.create_relayer_api_key(&relayer_id).await?;

    // Fund the relayer
    let middleware = setup_middleware(
        format!("http://{}", double_anvil.local_addr()),
        DEFAULT_ANVIL_PRIVATE_KEY,
    )
    .await?;

    let amount: U256 = parse_units("100", "ether")?.into();

    middleware
        .send_transaction(
            Eip1559TransactionRequest {
                to: Some(relayer_address.into()),
                value: Some(amount),
                ..Default::default()
            },
            None,
        )
        .await?
        .await?;

    let provider = middleware.provider();

    let current_balance = provider.get_balance(relayer_address, None).await?;
    assert_eq!(current_balance, amount);

    // Send a transaction
    let value: U256 = parse_units("1", "ether")?.into();
    client
        .send_tx(
            &api_key,
            &SendTxRequest {
                relayer_id,
                to: ARBITRARY_ADDRESS,
                value,
                gas_limit: U256::from(21_000),
                ..Default::default()
            },
        )
        .await?;

    for _ in 0..10 {
        let balance = provider.get_balance(ARBITRARY_ADDRESS, None).await?;

        if balance == value {
            return Ok(());
        } else {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    panic!("Transaction was not sent")
}
