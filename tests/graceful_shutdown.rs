mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::common::prelude::*;

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(20);

#[tokio::test]
async fn shutdown_joins_tasks_and_stops_server() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let shutdown = CancellationToken::new();
    let (service, _) = ServiceBuilder::default()
        .shutdown_timeout(SHUTDOWN_TIMEOUT)
        .shutdown(shutdown.clone())
        .build(&anvil, &db_url)
        .await?;
    let addr = service.local_addr();

    let has_task_finished = Arc::new(AtomicBool::new(false));
    let task_finished = has_task_finished.clone();
    service.task_runner().add_task("test_task", move |_, ctx| {
        let task_finished = task_finished.clone();
        async move {
            while !ctx.is_shutting_down() {
                ctx.idle(Duration::from_secs(1)).await;
            }
            task_finished.store(true, Ordering::SeqCst);

            Ok(())
        }
    });

    let (status, _) = http_get(addr, "/health/live").await?;
    assert_eq!(status, StatusCode::OK);

    shutdown.cancel();
    tokio::time::timeout(SHUTDOWN_TIMEOUT / 2, service.wait()).await??;

    assert!(has_task_finished.load(Ordering::SeqCst));
    assert!(http_get(addr, "/health/live").await.is_err());

    Ok(())
}
