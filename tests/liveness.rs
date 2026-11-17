mod common;

use crate::common::prelude::*;

const TASK_STALL_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn liveness_fails_once_a_task_stalls() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (service, _) = ServiceBuilder::default()
        .role(ServiceRole::Worker)
        .task_stall_timeout(TASK_STALL_TIMEOUT)
        .build(&anvil, &db_url)
        .await?;
    let addr = service.local_addr();

    let (status, _) = http_get(addr, "/health/live").await?;
    assert_eq!(status, StatusCode::OK);

    service
        .task_runner()
        .add_task("stalled_task", |_, _| std::future::pending());

    for path in ["/health/live", "/health"] {
        let body = await_status(
            addr,
            path,
            StatusCode::SERVICE_UNAVAILABLE,
            TASK_STALL_TIMEOUT * 3,
        )
        .await?;
        assert!(body.contains("stalled_task (Stalled)"), "{path}: {body}");
    }

    let (status, _) = http_get(addr, "/health/ready").await?;
    assert_eq!(status, StatusCode::OK, "readiness must not depend on tasks");

    Ok(())
}

/// Indexing that waits for a table lock stalls, because the lock is held for
/// longer than the stall timeout but not as long as the statement timeout.
#[tokio::test]
async fn liveness_fails_while_indexing_waits_for_a_lock() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (service, _) = ServiceBuilder::default()
        .role(ServiceRole::Worker)
        .task_stall_timeout(TASK_STALL_TIMEOUT)
        .build(&anvil, &db_url)
        .await?;
    let addr = service.local_addr();
    let db = connect_db(&db_url).await?;

    let mut lock = db.pool.begin().await?;
    sqlx::query("LOCK TABLE blocks IN EXCLUSIVE MODE")
        .execute(lock.as_mut())
        .await?;

    let body = await_status(
        addr,
        "/health/live",
        StatusCode::SERVICE_UNAVAILABLE,
        TASK_STALL_TIMEOUT * 3,
    )
    .await?;
    let stalled_indexing =
        format!("index_blocks/{DEFAULT_ANVIL_CHAIN_ID} (Stalled)");
    assert!(body.contains(&stalled_indexing), "{body}");

    lock.rollback().await?;
    await_status(addr, "/health/live", StatusCode::OK, TASK_STALL_TIMEOUT * 3)
        .await?;

    Ok(())
}
