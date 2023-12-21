mod common;

use ethers::prelude::*;
use tx_sitter::server::routes::relayer::CreateApiKeyResponse;
use url::Url;

use crate::common::prelude::*;

const ESCALATION_INTERVAL: Duration = Duration::from_secs(30);

#[tokio::test]
async fn rpc_access() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = setup_anvil(DEFAULT_ANVIL_BLOCK_TIME).await?;

    let (service, client) =
        setup_service(&anvil, &db_url, ESCALATION_INTERVAL).await?;

    let CreateApiKeyResponse { api_key } =
        client.create_relayer_api_key(DEFAULT_RELAYER_ID).await?;

    let rpc_url =
        format!("http://{}/1/api/{api_key}/rpc", service.local_addr());

    let provider = Provider::new(Http::new(rpc_url.parse::<Url>()?));

    let latest_block_number = provider.get_block_number().await?;

    let very_future_block = latest_block_number + 1000;
    let very_future_block = provider.get_block(very_future_block).await?;

    assert!(very_future_block.is_none());

    Ok(())
}
