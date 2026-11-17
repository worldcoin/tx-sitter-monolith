use alloy::primitives::U256;
use chrono::Utc;
use eyre::ContextCompat;

use self::gas_estimation::FeesEstimate;
use crate::app::App;
use crate::db::data::RelayerInfo;

pub mod gas_estimation;
pub mod l1_fees;
mod send;

pub use self::send::{
    send_raw_tx, SendFailure, SendOutcome, MAX_SEND_ATTEMPTS,
};

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

/// Returns whether `relayer` may broadcast and escalate transactions now.
///
/// It may not while it is disabled, while the gas price of its chain is above
/// one of its gas price limits, or while the L1 fee gate of its chain holds
/// transactions back, see [`l1_fees::L1FeeGate`].
pub async fn should_send_relayer_transactions(
    app: &App,
    relayer: &RelayerInfo,
) -> eyre::Result<bool> {
    if !relayer.enabled {
        tracing::warn!(
            relayer_id = relayer.id,
            relayer_address = %relayer.address,
            chain_id = relayer.chain_id,
            "Relayer is disabled, skipping transactions broadcast"
        );

        return Ok(false);
    }

    let l1_fee_gate = app.l1_fee_gate(relayer.chain_id);
    if relayer.gas_price_limits.is_empty() && l1_fee_gate.is_none() {
        return Ok(true);
    }

    let chain_fees = app
        .db
        .get_latest_block_fees_by_chain_id(relayer.chain_id)
        .await?;

    if let Some(l1_fee_gate) = l1_fee_gate {
        let l1_fees =
            chain_fees.as_ref().and_then(|fees| fees.l1_fees.as_ref());

        if !l1_fee_gate.allows(l1_fees, Utc::now()) {
            return Ok(false);
        }
    }

    for gas_limit in &relayer.gas_price_limits {
        let chain_fees = chain_fees.as_ref().context("Missing block")?;

        if chain_fees.gas_price > gas_limit.value.0 {
            tracing::warn!(
                relayer_id = relayer.id,
                relayer_address = %relayer.address,
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
