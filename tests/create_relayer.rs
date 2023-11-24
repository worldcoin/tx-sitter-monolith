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

    let (_service, client) =
        setup_service(&double_anvil, &db_url, ESCALATION_INTERVAL).await?;

    let CreateRelayerResponse { .. } = client
        .create_relayer(&CreateRelayerRequest {
            name: "Test relayer".to_string(),
            chain_id: DEFAULT_ANVIL_CHAIN_ID,
        })
        .await?;

    Ok(())
}
