mod common;

use tx_sitter_client::apis::admin_v1_api::UpdateRelayerParams;
use tx_sitter_client::apis::relayer_v1_api::CreateTransactionError;
use tx_sitter_client::apis::Error;

use crate::common::prelude::*;

const MAX_QUEUED_TXS: usize = 10;
const SENDS_PER_API: usize = 15;

#[tokio::test]
async fn send_too_many_txs() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    // Without a worker nothing leaves the queue
    let (_api_1, client_1) = ServiceBuilder::default()
        .role(ServiceRole::Api)
        .build(&anvil, &db_url)
        .await?;
    let (_api_2, client_2) = ServiceBuilder::default()
        .role(ServiceRole::Api)
        .build(&anvil, &db_url)
        .await?;
    let api_key = create_api_key(&client_1).await?;

    tx_sitter_client::apis::admin_v1_api::update_relayer(
        &client_1,
        UpdateRelayerParams {
            relayer_id: DEFAULT_RELAYER_ID.to_string(),
            relayer_update_request: RelayerUpdateRequest {
                max_queued_txs: Some(MAX_QUEUED_TXS as i32),
                ..Default::default()
            },
        },
    )
    .await?;

    let sends: FuturesUnordered<_> = [&client_1, &client_2]
        .into_iter()
        .flat_map(|client| {
            (0..SENDS_PER_API)
                .map(|_| send_transfer(client, &api_key, U256::from(1)))
        })
        .collect();
    let results: Vec<_> = sends.collect().await;

    let mut admitted = 0;
    for result in results {
        let Err(err) = result else {
            admitted += 1;
            continue;
        };

        match err.downcast_ref::<Error<CreateTransactionError>>() {
            Some(Error::ResponseError(response)) => {
                assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
                assert_eq!(response.content, "Relayer queue is full");
            }
            _ => return Err(err),
        }
    }

    assert_eq!(admitted, MAX_QUEUED_TXS);

    Ok(())
}
