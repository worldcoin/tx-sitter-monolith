use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;

use crate::app::App;

const PRUNING_INTERVAL: Duration = Duration::from_secs(60);

const fn minutes(seconds: i64) -> i64 {
    seconds * 60
}

const fn hours(seconds: i64) -> i64 {
    minutes(seconds) * 60
}

const fn days(seconds: i64) -> i64 {
    hours(seconds) * 24
}

const BLOCK_PRUNE_AGE_SECONDS: i64 = days(7);

pub async fn prune_blocks(app: Arc<App>) -> eyre::Result<()> {
    loop {
        let prune_age = chrono::Duration::seconds(BLOCK_PRUNE_AGE_SECONDS);
        let block_prune_timestamp = Utc::now() - prune_age;

        tracing::info!(?block_prune_timestamp, "Pruning blocks");

        app.db.prune_blocks(block_prune_timestamp).await?;

        tokio::time::sleep(PRUNING_INTERVAL).await;
    }
}
