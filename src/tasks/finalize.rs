use std::sync::Arc;
use std::time::Duration;

use crate::app::App;

const TIME_BETWEEN_FINALIZATIONS_SECONDS: i64 = 60;

pub async fn finalize_txs(app: Arc<App>) -> eyre::Result<()> {
    loop {
        let finalization_timestmap =
            chrono::Utc::now() - chrono::Duration::seconds(60 * 60);

        tracing::info!(
            "Finalizing txs mined before {}",
            finalization_timestmap
        );

        app.db.finalize_txs(finalization_timestmap).await?;

        tokio::time::sleep(Duration::from_secs(
            TIME_BETWEEN_FINALIZATIONS_SECONDS as u64,
        ))
        .await;
    }
}
