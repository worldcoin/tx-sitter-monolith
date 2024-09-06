mod common;

use tx_sitter_client::apis::admin_v1_api::CreateRelayerParams;

use crate::common::prelude::*;

#[tokio::test]
async fn create_relayer() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (_service, client) =
        ServiceBuilder::default().build(&anvil, &db_url).await?;

    let CreateRelayerResponse { .. } =
        tx_sitter_client::apis::admin_v1_api::create_relayer(
            &client,
            CreateRelayerParams {
                create_relayer_request: CreateRelayerRequest::new(
                    "Test relayer".to_string(),
                    DEFAULT_ANVIL_CHAIN_ID as i32,
                ),
            },
        )
        .await?;

    Ok(())
}
