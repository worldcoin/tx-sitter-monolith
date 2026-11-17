mod common;

use tx_sitter_client::apis::admin_v1_api::CreateNetworkParams;

use crate::common::prelude::*;

/// Networks are reconciled every 30 seconds
const NETWORK_PICKUP_TIMEOUT: Duration = Duration::from_secs(60);

#[tokio::test]
async fn network_created_twice_gets_one_set_of_tasks() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (service, client) = ServiceBuilder::default()
        .without_predefined_values()
        .build(&anvil, &db_url)
        .await?;

    for _ in 0..2 {
        tx_sitter_client::apis::admin_v1_api::create_network(
            &client,
            CreateNetworkParams {
                chain_id: DEFAULT_ANVIL_CHAIN_ID as i32,
                create_network_request: CreateNetworkRequest::new(
                    "Anvil".to_string(),
                    anvil.endpoint(),
                    anvil.ws_endpoint(),
                ),
            },
        )
        .await?;
    }

    await_estimates(&service, DEFAULT_ANVIL_CHAIN_ID, NETWORK_PICKUP_TIMEOUT)
        .await?;

    let labels: Vec<String> = service
        .task_runner()
        .task_health()
        .into_iter()
        .map(|(label, _)| label)
        .collect();
    let index_tasks: Vec<&String> = labels
        .iter()
        .filter(|label| label.starts_with("index_blocks/"))
        .collect();
    assert_eq!(
        index_tasks,
        vec![&format!("index_blocks/{DEFAULT_ANVIL_CHAIN_ID}")],
        "{labels:?}"
    );

    Ok(())
}
