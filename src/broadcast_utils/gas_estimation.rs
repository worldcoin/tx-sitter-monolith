use alloy::rpc::types::eth::FeeHistory as AlloyFeeHistory;
use ethers::types::U256;
use eyre::ContextCompat;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeesEstimate {
    pub base_fee_per_gas: U256,
    pub percentile_fees: Vec<U256>,
    pub base_fee_per_blob_gas: u128,
}

pub fn estimate_percentile_fees(
    fee_history: &AlloyFeeHistory,
) -> eyre::Result<FeesEstimate> {
    // Take the last base fee per gas
    let base_fee_per_gas = fee_history
        .base_fee_per_gas
        .last()
        .context("Missing base fees")?;

    let base_fee_per_blob_gas = fee_history
        .base_fee_per_blob_gas
        .last()
        .context("Missing base blob fees")?;

    let reward = fee_history
        .reward
        .as_ref()
        .ok_or_else(|| "Missing rewards".to_string())
        .unwrap();

    let num_percentiles = &reward.first().unwrap().len();
    let percentile_fees = (0..*num_percentiles)
        .map(|percentile_idx| estimate_percentile(reward, percentile_idx))
        .collect::<eyre::Result<Vec<_>>>()?;

    Ok(FeesEstimate {
        base_fee_per_gas: U256::from(*base_fee_per_gas),
        percentile_fees,
        base_fee_per_blob_gas: *base_fee_per_blob_gas,
    })
}

// Just takes the average of the percentile fees
fn estimate_percentile(
    reward: &Vec<Vec<u128>>,
    percentile_idx: usize,
) -> eyre::Result<U256> {
    let mut sum: u128 = 0;

    for rewards in reward {
        let percentile_fee = rewards
            .get(percentile_idx)
            .context("Missing percentile fee")?;

        sum += percentile_fee;
    }

    let percentile_fee = sum / reward.len() as u128;

    Ok(U256::from(percentile_fee))
}

#[cfg(test)]
mod tests {
    use alloy::rpc::types::eth::FeeHistory as AlloyFeeHistory;
    use ethers::types::U256;

    use super::*;

    // Below are really just sample calculations

    #[test]
    fn estimate_fees_optimism_108598243() {
        const CONTENT: &str = include_str!(
            "./gas_estimation/fee_history_optimism_108598243.json"
        );

        let fee_history: AlloyFeeHistory =
            serde_json::from_str(CONTENT).unwrap();

        let estimates = estimate_percentile_fees(&fee_history).unwrap();
        let expected_estimates = FeesEstimate {
            base_fee_per_gas: U256::from(87),
            percentile_fees: vec![U256::from(12), U256::from(28504848)],
            base_fee_per_blob_gas: 87,
        };

        assert_eq!(expected_estimates, estimates);
    }

    #[test]
    fn estimate_fees_history_ethereum_17977856() {
        const CONTENT: &str =
            include_str!("./gas_estimation/fee_history_ethereum_17977856.json");

        let fee_history: AlloyFeeHistory =
            serde_json::from_str(CONTENT).unwrap();

        let estimates = estimate_percentile_fees(&fee_history).unwrap();
        let expected_estimates = FeesEstimate {
            base_fee_per_gas: U256::from(23149734459u64),
            percentile_fees: vec![
                U256::from(42461472u64),
                U256::from(100000000u64),
                U256::from(613437626u64),
                U256::from(2152582987u64),
            ],
            base_fee_per_blob_gas: 23149734459,
        };

        assert_eq!(expected_estimates, estimates);
    }
}
