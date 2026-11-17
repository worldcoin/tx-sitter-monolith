mod common;

use std::sync::Arc;

use tokio::sync::Notify;

use crate::common::prelude::*;

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

#[tokio::test]
async fn shutdown_gives_up_on_tasks_after_timeout() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let shutdown = CancellationToken::new();
    let (service, _) = ServiceBuilder::default()
        .shutdown_timeout(SHUTDOWN_TIMEOUT)
        .shutdown(shutdown.clone())
        .build(&anvil, &db_url)
        .await?;

    let task_started = Arc::new(Notify::new());
    let started = task_started.clone();
    service
        .task_runner()
        .add_task("ignores_shutdown", move |_, _| {
            started.notify_one();
            std::future::pending()
        });
    task_started.notified().await;

    shutdown.cancel();
    let started_at = tokio::time::Instant::now();
    tokio::time::timeout(SHUTDOWN_TIMEOUT * 3, service.wait()).await??;

    assert!(started_at.elapsed() >= SHUTDOWN_TIMEOUT);

    Ok(())
}
