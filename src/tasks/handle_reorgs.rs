use std::sync::Arc;

use crate::app::App;

pub async fn handle_hard_reorgs(app: Arc<App>) -> eyre::Result<()> {
    loop {
        tracing::info!("Handling hard reorgs");

        let reorged_txs = app.db.handle_hard_reorgs().await?;

        for tx in reorged_txs {
            tracing::info!(tx_id = tx, "Transaction hard reorged");
        }

        tokio::time::sleep(app.config.service.hard_reorg_interval).await;
    }
}

pub async fn handle_soft_reorgs(app: Arc<App>) -> eyre::Result<()> {
    loop {
        tracing::info!("Handling soft reorgs");

        let txs = app.db.handle_soft_reorgs().await?;

        for tx in txs {
            tracing::info!(tx_id = tx, "Transaction soft reorged");
        }

        tokio::time::sleep(app.config.service.soft_reorg_interval).await;
    }
}
