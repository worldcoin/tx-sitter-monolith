mod common;

use crate::common::prelude::*;

const TASK_STALL_TIMEOUT: Duration = Duration::from_secs(5);
const RPC_TIMEOUT: Duration = Duration::from_secs(1);

/// An unresponsive WS RPC makes indexing time out and restart. That is a
/// degraded mode, so it must neither restart the pod nor take it out of
/// service.
#[tokio::test]
async fn hanging_block_stream_fails_neither_probe() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    let hanging_ws_url = format!("ws://{}", spawn_unresponsive_rpc().await?);

    let db = connect_db(&db_url).await?;
    db.upsert_network(
        DEFAULT_ANVIL_CHAIN_ID,
        "Anvil",
        &anvil.endpoint(),
        &hanging_ws_url,
    )
    .await?;

    let (service, _) = ServiceBuilder::default()
        .without_predefined_values()
        .rpc_timeout(RPC_TIMEOUT)
        .task_stall_timeout(TASK_STALL_TIMEOUT)
        .build(&anvil, &db_url)
        .await?;
    let addr = service.local_addr();

    let deadline = tokio::time::Instant::now() + TASK_STALL_TIMEOUT * 3;
    while tokio::time::Instant::now() < deadline {
        for path in ["/health/live", "/health/ready"] {
            let (status, body) = http_get(addr, path).await?;
            assert_eq!(status, StatusCode::OK, "{path}: {body}");
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let latest_block =
        db.get_latest_block_number(DEFAULT_ANVIL_CHAIN_ID).await?;
    assert_eq!(latest_block, None, "blocks were indexed");

    Ok(())
}
