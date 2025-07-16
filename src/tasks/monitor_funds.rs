use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ethers::providers::Middleware;
use ethers::types::{NameOrAddress, U256};

use crate::app::App;

const INTERVAL: Duration = Duration::from_secs(15);
const LOW_BALANCE_THRESHOLD: u64 = 1000000000000000000; // 1 ETH
const REPORTING_INTERVAL: Duration = Duration::from_secs(60);

pub async fn monitor_funds(app: Arc<App>) -> eyre::Result<()> {
    let low_balance_threshold = U256::from(LOW_BALANCE_THRESHOLD);

    let mut reporting_cache = HashMap::new();

    loop {
        let relayers = app.db.get_relayers().await?;

        for relayer in &relayers {
            if let Some(last_time_reported) = reporting_cache.get(&relayer.id) {
                let diff = Instant::now() - *last_time_reported;

                if diff < REPORTING_INTERVAL {
                    continue;
                }
            }
            reporting_cache.insert(relayer.id.clone(), Instant::now());

            let provider = app.http_provider(relayer.chain_id).await?;

            let balance = provider
                .get_balance(NameOrAddress::Address(relayer.address.0), None)
                .await?;

            if balance < low_balance_threshold {
                tracing::warn!(
                    relayer_id = relayer.id,
                    address = %relayer.address.0,
                    balance = %balance,
                    "Relayer has low balance"
                );
            }
        }

        tokio::time::sleep(INTERVAL).await;
    }
}
