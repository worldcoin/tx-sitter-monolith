use ethers::types::U256;
use eyre::ContextCompat;

use self::gas_estimation::FeesEstimate;
use crate::app::App;
use crate::types::RelayerInfo;

pub mod gas_estimation;

/// Returns a tuple of max and max priority fee per gas
pub fn calculate_gas_fees_from_estimates(
    estimates: &FeesEstimate,
    tx_priority_index: usize,
    max_base_fee_per_gas: U256,
) -> (U256, U256) {
    let max_priority_fee_per_gas = estimates.percentile_fees[tx_priority_index];

    let max_fee_per_gas = max_base_fee_per_gas + max_priority_fee_per_gas;

    (max_fee_per_gas, max_priority_fee_per_gas)
}

pub async fn should_send_relayer_transactions(
    app: &App,
    relayer: &RelayerInfo,
) -> eyre::Result<bool> {
    if !relayer.enabled {
        tracing::warn!(
            relayer_id = relayer.id,
            chain_id = relayer.chain_id,
            "Relayer is disabled, skipping transactions broadcast"
        );

        return Ok(false);
    }

    for gas_limit in &relayer.gas_price_limits.0 {
        let chain_fees = app
            .db
            .get_latest_block_fees_by_chain_id(relayer.chain_id)
            .await?
            .context("Missing block")?;

        if chain_fees.gas_price > gas_limit.value.0 {
            tracing::warn!(
                relayer_id = relayer.id,
                chain_id = relayer.chain_id,
                gas_price = ?chain_fees.gas_price,
                gas_limit = ?gas_limit.value.0,
                "Gas price is too high for relayer"
            );

            return Ok(false);
        }
    }

    Ok(true)
}
