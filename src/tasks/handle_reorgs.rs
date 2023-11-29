use std::sync::Arc;
use std::time::Duration;

use crate::app::App;

// TODO: Make this configurable
const TIME_BETWEEN_HARD_REORGS_SECONDS: i64 = 60 * 60; // Once every hour
const TIME_BETWEEN_SOFT_REORGS_SECONDS: i64 = 60; // Once every minute

pub async fn handle_hard_reorgs(app: Arc<App>) -> eyre::Result<()> {
    loop {
        tracing::info!("Handling hard reorgs");

        app.db.handle_hard_reorgs().await?;

        tokio::time::sleep(Duration::from_secs(
            TIME_BETWEEN_HARD_REORGS_SECONDS as u64,
        ))
        .await;
    }
}

pub async fn handle_soft_reorgs(app: Arc<App>) -> eyre::Result<()> {
    loop {
        tracing::info!("Handling soft reorgs");

        app.db.handle_soft_reorgs().await?;

        tokio::time::sleep(Duration::from_secs(
            TIME_BETWEEN_SOFT_REORGS_SECONDS as u64,
        ))
        .await;
    }
}