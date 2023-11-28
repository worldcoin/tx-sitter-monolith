use ethers::types::{Eip1559TransactionRequest, U256};
use eyre::ContextCompat;

use self::gas_estimation::FeesEstimate;
use crate::app::App;

pub mod gas_estimation;

const BASE_FEE_PER_GAS_SURGE_FACTOR: u64 = 2;

// TODO: Adjust
const MIN_PRIORITY_FEE: U256 = U256([10, 0, 0, 0]);
const MAX_GAS_PRICE: U256 = U256([100_000_000_000, 0, 0, 0]);

/// Returns a tuple of max and max priority fee per gas
pub fn calculate_gas_fees_from_estimates(
    estimates: &FeesEstimate,
    tx_priority_index: usize,
    max_base_fee_per_gas: U256,
) -> eyre::Result<(U256, U256)> {
    let max_priority_fee_per_gas = estimates.percentile_fees[tx_priority_index];

    let max_priority_fee_per_gas =
        std::cmp::max(max_priority_fee_per_gas, MIN_PRIORITY_FEE);

    let max_fee_per_gas = max_base_fee_per_gas + max_priority_fee_per_gas;
    let max_fee_per_gas = std::cmp::min(max_fee_per_gas, MAX_GAS_PRICE);

    Ok((max_fee_per_gas, max_priority_fee_per_gas))
}

/// Calculates the max base fee per gas
/// Returns an error if the base fee per gas is too high
///
/// i.e. the base fee from estimates surged by a factor
pub fn calculate_max_base_fee_per_gas(
    estimates: &FeesEstimate,
) -> eyre::Result<U256> {
    let base_fee_per_gas = estimates.base_fee_per_gas;

    if base_fee_per_gas > MAX_GAS_PRICE {
        tracing::warn!("Base fee per gas is too high, retrying later");
        eyre::bail!("Base fee per gas is too high");
    }

    // Surge the base fee per gas
    let max_base_fee_per_gas = base_fee_per_gas * BASE_FEE_PER_GAS_SURGE_FACTOR;

    Ok(max_base_fee_per_gas)
}

pub fn escalate_priority_fee(
    max_base_fee_per_gas: U256,
    max_network_fee_per_gas: U256,
    current_max_priority_fee_per_gas: U256,
    escalation_count: usize,
    tx: &mut Eip1559TransactionRequest,
) {
    // Min increase of 20% on the priority fee required for a replacement tx
    let increased_gas_price_percentage =
        U256::from(100 + (10 * (1 + escalation_count)));

    let factor = U256::from(100);

    let new_max_priority_fee_per_gas = current_max_priority_fee_per_gas
        * increased_gas_price_percentage
        / factor;

    let new_max_priority_fee_per_gas =
        std::cmp::min(new_max_priority_fee_per_gas, max_network_fee_per_gas);

    let new_max_fee_per_gas =
        max_base_fee_per_gas + new_max_priority_fee_per_gas;
    let new_max_fee_per_gas =
        std::cmp::min(new_max_fee_per_gas, max_network_fee_per_gas);

    tx.max_fee_per_gas = Some(new_max_fee_per_gas);
    tx.max_priority_fee_per_gas = Some(new_max_priority_fee_per_gas);
}

pub async fn should_send_transaction(
    app: &App,
    relayer_id: &str,
) -> eyre::Result<bool> {
    let relayer = app.db.get_relayer(relayer_id).await?;

    for gas_limit in &relayer.gas_limits.0 {
        let chain_fees = app
            .db
            .get_latest_block_fees_by_chain_id(relayer.chain_id)
            .await?
            .context("Missing block")?;

        tracing::info!(?chain_fees, gas_limit = ?gas_limit.value.0, "Checking gas price",);

        if chain_fees.gas_price > gas_limit.value.0 {
            tracing::warn!(
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
