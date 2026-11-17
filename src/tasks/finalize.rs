use std::sync::Arc;
use std::time::Duration;

use crate::app::App;
use crate::task_runner::TaskContext;

const TIME_BETWEEN_FINALIZATIONS_SECONDS: i64 = 60;

pub async fn finalize_txs(app: Arc<App>, ctx: TaskContext) -> eyre::Result<()> {
    while !ctx.is_shutting_down() {
        let finalization_timestamp =
            chrono::Utc::now() - chrono::Duration::seconds(60 * 60);

        tracing::info!(
            "Finalizing txs mined before {}",
            finalization_timestamp
        );

        let finalized_txs = app.db.finalize_txs(finalization_timestamp).await?;
        metrics::counter!("tx_finalized").increment(finalized_txs.len() as u64);

        ctx.idle(Duration::from_secs(
            TIME_BETWEEN_FINALIZATIONS_SECONDS as u64,
        ))
        .await;
    }

    Ok(())
}
