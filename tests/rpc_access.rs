mod common;

use crate::common::prelude::*;

#[tokio::test]
async fn rpc_access() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (service, _) = ServiceBuilder::default().build(&anvil, &db_url).await?;

    let api_key = DEFAULT_API_KEY.to_string();

    let rpc_url =
        format!("http://{}/1/api/{}/rpc", service.local_addr(), api_key);

    let provider: RootProvider =
        RootProvider::new_http(rpc_url.parse::<Url>()?);

    let latest_block_number = provider.get_block_number().await?;

    let very_future_block = latest_block_number + 1000;
    let very_future_block = provider
        .get_block_by_number(very_future_block.into())
        .await?;

    assert!(very_future_block.is_none());

    Ok(())
}
