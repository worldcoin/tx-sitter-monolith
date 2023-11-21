use ethers::types::{FeeHistory, U256};
use eyre::ContextCompat;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeesEstimate {
    pub base_fee_per_gas: U256,
    pub percentile_fees: Vec<U256>,
}

pub fn estimate_percentile_fees(
    fee_history: &FeeHistory,
) -> eyre::Result<FeesEstimate> {
    // Take the last base fee per gas
    let base_fee_per_gas = fee_history
        .base_fee_per_gas
        .last()
        .context("Missing base fees")?;

    let num_percentiles =
        fee_history.reward.first().context("Missing rewards")?.len();
    let percentile_fees = (0..num_percentiles)
        .map(|percentile_idx| estimate_percentile(fee_history, percentile_idx))
        .collect::<eyre::Result<Vec<_>>>()?;

    Ok(FeesEstimate {
        base_fee_per_gas: *base_fee_per_gas,
        percentile_fees,
    })
}

// Just takes the average of the percentile fees
fn estimate_percentile(
    fee_history: &FeeHistory,
    percentile_idx: usize,
) -> eyre::Result<U256> {
    let mut sum = U256::zero();

    for rewards in &fee_history.reward {
        let percentile_fee = rewards
            .get(percentile_idx)
            .context("Missing percentile fee")?;

        sum += *percentile_fee;
    }

    let percentile_fee = sum / fee_history.reward.len();

    Ok(percentile_fee)
}

#[cfg(test)]
mod tests {
    use ethers::types::{FeeHistory, U256};

    use super::*;

    // Below are really just sample calculations

    #[test]
    fn estimate_fees_optimism_108598243() {
        const CONTENT: &str = include_str!(
            "./gas_estimation/fee_history_optimism_108598243.json"
        );

        let fee_history: FeeHistory = serde_json::from_str(CONTENT).unwrap();

        let estimates = estimate_percentile_fees(&fee_history).unwrap();
        let expected_estimates = FeesEstimate {
            base_fee_per_gas: U256::from(87),
            percentile_fees: vec![U256::from(12), U256::from(28504848)],
        };

        assert_eq!(expected_estimates, estimates);
    }

    #[test]
    fn estimate_fees_history_ethereum_17977856() {
        const CONTENT: &str =
            include_str!("./gas_estimation/fee_history_ethereum_17977856.json");

        let fee_history: FeeHistory = serde_json::from_str(CONTENT).unwrap();

        let estimates = estimate_percentile_fees(&fee_history).unwrap();
        let expected_estimates = FeesEstimate {
            base_fee_per_gas: U256::from(23149734459u64),
            percentile_fees: vec![
                U256::from(42461472u64),
                U256::from(100000000u64),
                U256::from(613437626u64),
                U256::from(2152582987u64),
            ],
        };

        assert_eq!(expected_estimates, estimates);
    }
}
