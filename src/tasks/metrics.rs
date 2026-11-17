use std::sync::Arc;
use std::time::Duration;

use crate::app::App;
use crate::task_runner::TaskContext;

const EMIT_METRICS_INTERVAL: Duration = Duration::from_secs(1);

pub async fn emit_metrics(app: Arc<App>, ctx: TaskContext) -> eyre::Result<()> {
    while !ctx.is_shutting_down() {
        for network in &app.config.networks {
            let chain_id = network.chain_id;
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

            if let Some((block_number, timestamp)) =
                app.db.get_latest_block(chain_id).await?
            {
                let age = chrono::Utc::now() - timestamp;

                metrics::gauge!("indexed_block_number", &labels)
                    .set(block_number as f64);
                metrics::gauge!("indexed_block_age_seconds", &labels)
                    .set(age.num_milliseconds() as f64 / 1000.0);
            }

            if let Some(l1_fee_gate) = app.l1_fee_gate(chain_id) {
                let held_txs = if l1_fee_gate.is_holding() {
                    stats.pending_txs
                } else {
                    0
                };
                metrics::gauge!("l1_fee_held_txs", &labels)
                    .set(held_txs as f64);
            }
        }

        let tx_events_lag = app.db.get_tx_events_lag().await?;
        metrics::gauge!("tx_events_held_back_seconds")
            .set(tx_events_lag.held_back_seconds);
        metrics::gauge!("notification_queue_usage")
            .set(tx_events_lag.notification_queue_usage);

        ctx.idle(EMIT_METRICS_INTERVAL).await;
    }

    Ok(())
}
