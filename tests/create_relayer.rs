use std::time::Duration;

use service::server::data::{CreateRelayerRequest, CreateRelayerResponse};

use crate::common::*;

mod common;

const ESCALATION_INTERVAL: Duration = Duration::from_secs(30);

#[tokio::test]
async fn create_relayer() -> eyre::Result<()> {
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

    let _response: CreateRelayerResponse = response.json().await?;

    Ok(())
}
