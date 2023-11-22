use std::time::Duration;

use ethers::providers::Middleware;
use ethers::types::{Eip1559TransactionRequest, U256};
use ethers::utils::parse_units;
use service::server::data::{
    CreateRelayerRequest, CreateRelayerResponse, SendTxRequest, SendTxResponse,
};

mod common;

use crate::common::*;

const ESCALATION_INTERVAL: Duration = Duration::from_secs(30);

#[tokio::test]
async fn send_many_txs() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let double_anvil = setup_double_anvil().await?;

    let service =
        setup_service(&double_anvil.local_addr(), &db_url, ESCALATION_INTERVAL)
            .await?;

    let addr = service.local_addr();

    let response = reqwest::Client::new()
        .post(&format!("http://{}/1/relayer/create", addr))
        .json(&CreateRelayerRequest {
            name: "Test relayer".to_string(),
            chain_id: DEFAULT_ANVIL_CHAIN_ID,
        })
        .send()
        .await?;

    let response: CreateRelayerResponse = response.json().await?;

    // Fund the relayer
    let middleware = setup_middleware(
        format!("http://{}", double_anvil.local_addr()),
        DEFAULT_ANVIL_PRIVATE_KEY,
    )
    .await?;

    let amount: U256 = parse_units("1000", "ether")?.into();

    middleware
        .send_transaction(
            Eip1559TransactionRequest {
                to: Some(response.address.into()),
                value: Some(amount),
                ..Default::default()
            },
            None,
        )
        .await?
        .await?;

    let provider = middleware.provider();

    let current_balance = provider.get_balance(response.address, None).await?;
    assert_eq!(current_balance, amount);

    // Send a transaction
    let value: U256 = parse_units("10", "ether")?.into();
    let num_transfers = 10;
    let relayer_id = response.relayer_id;

    for _ in 0..num_transfers {
        let response = reqwest::Client::new()
            .post(&format!("http://{}/1/tx/send", addr))
            .json(&SendTxRequest {
                relayer_id: relayer_id.clone(),
                to: ARBITRARY_ADDRESS,
                value,
                gas_limit: U256::from(21_000),
                ..Default::default()
            })
            .send()
            .await?;

        let _response: SendTxResponse = response.json().await?;
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
