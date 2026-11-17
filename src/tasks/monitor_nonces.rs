use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use crate::app::App;
use crate::task_runner::TaskContext;

const INTERVAL: Duration = Duration::from_secs(15);

/// Reports the [stale transactions](crate::db::Database::count_stale_txs) of
/// every relayer, which its key sending transactions outside tx-sitter
/// leaves behind.
///
/// Their count per relayer is the `relayer_stale_txs` gauge, and a relayer
/// whose count becomes non-zero is logged at error once. Stale transactions
/// are never broadcast or escalated. Resetting the relayer deletes the unsent
/// ones; the sent ones stay pending.
pub async fn monitor_nonces(
    app: Arc<App>,
    ctx: TaskContext,
) -> eyre::Result<()> {
    let mut relayers_with_stale_txs = HashSet::new();

    while !ctx.is_shutting_down() {
        for relayer in app.db.count_stale_txs().await? {
            metrics::gauge!(
                "relayer_stale_txs",
                &[
                    ("relayer_id", relayer.relayer_id.clone()),
                    ("chain_id", relayer.chain_id.to_string()),
                ]
            )
            .set(relayer.stale_txs as f64);

            if relayer.stale_txs == 0 {
                relayers_with_stale_txs.remove(&relayer.relayer_id);
            } else if relayers_with_stale_txs.insert(relayer.relayer_id.clone())
            {
                tracing::error!(
                    relayer_id = relayer.relayer_id,
                    chain_id = relayer.chain_id,
                    current_nonce = relayer.current_nonce,
                    stale_txs = relayer.stale_txs,
                    "Relayer has transactions whose nonce the chain already used, reset the relayer to delete the unsent ones"
                );
            }
        }

        ctx.idle(INTERVAL).await;
    }

    Ok(())
}
