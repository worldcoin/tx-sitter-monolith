use std::sync::Arc;

use crate::app::App;
use crate::task_runner::TaskContext;

pub async fn handle_hard_reorgs(
    app: Arc<App>,
    ctx: TaskContext,
) -> eyre::Result<()> {
    while !ctx.is_shutting_down() {
        tracing::info!("Handling hard reorgs");

        let reorged_txs = app.db.handle_hard_reorgs().await?;

        for tx in reorged_txs {
            tracing::info!(tx_id = tx, "Transaction hard reorged");
        }

        ctx.idle(app.config.service.hard_reorg_interval).await;
    }

    Ok(())
}

pub async fn handle_soft_reorgs(
    app: Arc<App>,
    ctx: TaskContext,
) -> eyre::Result<()> {
    while !ctx.is_shutting_down() {
        tracing::info!("Handling soft reorgs");

        let txs = app.db.handle_soft_reorgs().await?;

        for tx in txs {
            tracing::info!(tx_id = tx, "Transaction soft reorged");
        }

        ctx.idle(app.config.service.soft_reorg_interval).await;
    }

    Ok(())
}
