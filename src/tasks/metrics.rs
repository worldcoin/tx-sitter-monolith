use std::sync::Arc;
use std::time::Duration;

use crate::app::App;

const EMIT_METRICS_INTERVAL: Duration = Duration::from_secs(1);

pub async fn emit_metrics(app: Arc<App>) -> eyre::Result<()> {
    loop {
        let chain_ids = app.db.get_network_chain_ids().await?;

        for chain_id in chain_ids {
            let stats = app.db.get_stats(chain_id).await?;

            // TODO: Add labels for env, etc.
            let labels = [("chain_id", chain_id.to_string())];

            metrics::gauge!("pending_txs", &labels)
                .set(stats.pending_txs as f64);
            metrics::gauge!("mined_txs", &labels).set(stats.mined_txs as f64);
            metrics::gauge!("finalized_txs", &labels)
                .set(stats.finalized_txs as f64);
            metrics::gauge!("total_indexed_blocks", &labels)
                .set(stats.total_indexed_blocks as f64);
            metrics::gauge!("block_txs", &labels).set(stats.block_txs as f64);
        }

        tokio::time::sleep(EMIT_METRICS_INTERVAL).await;
    }
}
