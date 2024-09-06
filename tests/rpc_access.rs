mod common;

use tx_sitter_client::apis::admin_v1_api::RelayerCreateApiKeyParams;

use crate::common::prelude::*;

#[tokio::test]
async fn rpc_access() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (service, client) =
        ServiceBuilder::default().build(&anvil, &db_url).await?;

    let CreateApiKeyResponse { api_key } =
        tx_sitter_client::apis::admin_v1_api::relayer_create_api_key(
            &client,
            RelayerCreateApiKeyParams {
                relayer_id: DEFAULT_RELAYER_ID.to_string(),
            },
        )
        .await?;

    let rpc_url =
        format!("http://{}/1/api/{}/rpc", service.local_addr(), api_key);

    let provider = Provider::new(Http::new(rpc_url.parse::<Url>()?));

    let latest_block_number = provider.get_block_number().await?;

    let very_future_block = latest_block_number + 1000;
    let very_future_block = provider.get_block(very_future_block).await?;

    assert!(very_future_block.is_none());

    Ok(())
}
