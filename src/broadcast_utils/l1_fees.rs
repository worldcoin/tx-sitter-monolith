use std::sync::{Mutex, PoisonError};

use alloy::eips::BlockNumberOrTag;
use alloy::primitives::{address, Address, U256};
use alloy::providers::{Provider, RootProvider};
use alloy::rpc::types::TransactionRequest;
use alloy::sol;
use alloy::sol_types::SolCall;
use chrono::{DateTime, Utc};
use eyre::Context;
use serde::{Deserialize, Serialize};

use crate::config::OpStackConfig;

/// The address of the `GasPriceOracle` predeploy on every OP-stack chain
pub const GAS_PRICE_ORACLE: Address =
    address!("420000000000000000000000000000000000000F");

sol! {
    /// The L1 fees of an OP-stack chain, as its `GasPriceOracle` predeploy
    /// reports them
    interface GasPriceOracle {
        function l1BaseFee() external view returns (uint256);
        function blobBaseFee() external view returns (uint256);
    }
}

/// The L1 fees an OP-stack chain charges for posting transaction data to L1,
/// in wei
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct L1Fees {
    pub l1_base_fee: U256,
    pub blob_base_fee: U256,
    /// When the fees were read from the oracle
    pub read_at: DateTime<Utc>,
}

/// Reads the L1 fees of the chain behind `rpc` from its `GasPriceOracle`
/// predeploy, at the latest block.
///
/// Fails if either call fails, or returns something other than a `uint256`,
/// for example on a chain without the predeploy.
pub async fn read_l1_fees(
    rpc: &RootProvider,
    now: DateTime<Utc>,
) -> eyre::Result<L1Fees> {
    let (l1_base_fee, blob_base_fee) = tokio::try_join!(
        call_oracle(rpc, GasPriceOracle::l1BaseFeeCall {}),
        call_oracle(rpc, GasPriceOracle::blobBaseFeeCall {}),
    )?;

    Ok(L1Fees {
        l1_base_fee,
        blob_base_fee,
        read_at: now,
    })
}

async fn call_oracle<C>(rpc: &RootProvider, call: C) -> eyre::Result<U256>
where
    C: SolCall<Return = U256>,
{
    let request = TransactionRequest::default()
        .to(GAS_PRICE_ORACLE)
        .input(call.abi_encode().into());

    let output = rpc
        .call(request)
        .block(BlockNumberOrTag::Latest.into())
        .await
        .with_context(|| format!("Calling GasPriceOracle.{}", C::SIGNATURE))?;

    C::abi_decode_returns(&output).with_context(|| {
        format!("Decoding the result of GasPriceOracle.{}", C::SIGNATURE)
    })
}

/// Why an OP-stack chain's transactions are held back
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum L1FeeHold {
    /// No L1 fees were read within `max_fee_age`
    Unknown,
    L1BaseFeeAboveCap,
    BlobBaseFeeAboveCap,
}

/// Returns why the L1 fees last read, `fees`, hold back transactions at
/// `now`, or `None` if they let transactions go out.
///
/// A fee equal to its cap lets transactions go out. Fees read longer than
/// `caps.max_fee_age` ago count as unknown, the same as no fees.
pub fn l1_fee_hold(
    caps: &OpStackConfig,
    fees: Option<&L1Fees>,
    now: DateTime<Utc>,
) -> Option<L1FeeHold> {
    let Some(fees) = fees else {
        return Some(L1FeeHold::Unknown);
    };

    let is_stale = (now - fees.read_at)
        .to_std()
        .is_ok_and(|age| age > caps.max_fee_age);

    if is_stale {
        Some(L1FeeHold::Unknown)
    } else if fees.l1_base_fee > caps.max_l1_base_fee.0 {
        Some(L1FeeHold::L1BaseFeeAboveCap)
    } else if fees.blob_base_fee > caps.max_blob_base_fee.0 {
        Some(L1FeeHold::BlobBaseFeeAboveCap)
    } else {
        None
    }
}

/// The L1 fee gate of an OP-stack chain, see [`OpStackConfig`].
///
/// It remembers whether it holds back the chain's transactions, so that it
/// logs at warn only when that changes rather than on every check.
pub struct L1FeeGate {
    chain_id: u64,
    caps: OpStackConfig,
    hold: Mutex<Option<L1FeeHold>>,
}

impl L1FeeGate {
    /// Creates the gate of `chain_id`, which starts out letting transactions
    /// go out.
    pub fn new(chain_id: u64, caps: OpStackConfig) -> Self {
        Self {
            chain_id,
            caps,
            hold: Mutex::new(None),
        }
    }

    /// Returns whether the chain's transactions may go out given the L1 fees
    /// last read, `fees`, at `now`, see [`l1_fee_hold`].
    pub fn allows(&self, fees: Option<&L1Fees>, now: DateTime<Utc>) -> bool {
        let hold = l1_fee_hold(&self.caps, fees, now);

        let previous_hold = std::mem::replace(
            &mut *self.hold.lock().unwrap_or_else(PoisonError::into_inner),
            hold,
        );

        if hold != previous_hold {
            let message = if hold.is_some() {
                "Holding transactions because of L1 fees"
            } else {
                "No longer holding transactions because of L1 fees"
            };

            tracing::warn!(
                chain_id = self.chain_id,
                ?hold,
                ?previous_hold,
                l1_base_fee = ?fees.map(|fees| fees.l1_base_fee),
                blob_base_fee = ?fees.map(|fees| fees.blob_base_fee),
                read_at = ?fees.map(|fees| fees.read_at),
                max_l1_base_fee = %self.caps.max_l1_base_fee.0,
                max_blob_base_fee = %self.caps.max_blob_base_fee.0,
                max_fee_age = ?self.caps.max_fee_age,
                "{message}"
            );
        }

        hold.is_none()
    }

    /// Returns whether the gate held back transactions at its last check.
    pub fn is_holding(&self) -> bool {
        self.hold
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use base_api_types::DecimalU256;
    use chrono::TimeZone;
    use test_case::test_case;

    use super::*;

    const MAX_L1_BASE_FEE: u64 = 200_000_000;
    const MAX_BLOB_BASE_FEE: u64 = 20_000_000;
    const MAX_FEE_AGE: Duration = Duration::from_secs(120);

    fn caps() -> OpStackConfig {
        OpStackConfig {
            max_l1_base_fee: DecimalU256(U256::from(MAX_L1_BASE_FEE)),
            max_blob_base_fee: DecimalU256(U256::from(MAX_BLOB_BASE_FEE)),
            max_fee_age: MAX_FEE_AGE,
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 17, 12, 0, 0).unwrap()
    }

    /// Worldchain's L1 fees on 2026-11-17, read `age` before [`now`]
    fn fees(age: Duration) -> L1Fees {
        L1Fees {
            l1_base_fee: U256::from(168_039_652),
            blob_base_fee: U256::from(10_274_004),
            read_at: now() - age,
        }
    }

    #[test_case(fees(Duration::ZERO), None ; "below the caps")]
    #[test_case(
        L1Fees { l1_base_fee: U256::from(MAX_L1_BASE_FEE), blob_base_fee: U256::from(MAX_BLOB_BASE_FEE), ..fees(Duration::ZERO) },
        None ;
        "at the caps"
    )]
    #[test_case(
        L1Fees { l1_base_fee: U256::from(MAX_L1_BASE_FEE + 1), ..fees(Duration::ZERO) },
        Some(L1FeeHold::L1BaseFeeAboveCap) ;
        "l1 base fee above its cap"
    )]
    #[test_case(
        L1Fees { blob_base_fee: U256::from(MAX_BLOB_BASE_FEE + 1), ..fees(Duration::ZERO) },
        Some(L1FeeHold::BlobBaseFeeAboveCap) ;
        "blob base fee above its cap"
    )]
    #[test_case(fees(MAX_FEE_AGE), None ; "read max_fee_age ago")]
    #[test_case(
        fees(MAX_FEE_AGE + Duration::from_secs(1)),
        Some(L1FeeHold::Unknown) ;
        "read longer than max_fee_age ago"
    )]
    #[test_case(
        L1Fees { l1_base_fee: U256::from(MAX_L1_BASE_FEE + 1), ..fees(MAX_FEE_AGE + Duration::from_secs(1)) },
        Some(L1FeeHold::Unknown) ;
        "stale fees above a cap"
    )]
    #[test_case(
        L1Fees { read_at: now() + chrono::Duration::seconds(5), ..fees(Duration::ZERO) },
        None ;
        "read after now"
    )]
    fn holds_while_a_fee_is_above_its_cap_or_stale(
        fees: L1Fees,
        expected: Option<L1FeeHold>,
    ) {
        assert_eq!(l1_fee_hold(&caps(), Some(&fees), now()), expected);
    }

    #[test]
    fn holds_without_fees() {
        assert_eq!(l1_fee_hold(&caps(), None, now()), Some(L1FeeHold::Unknown));
    }

    #[test]
    fn gate_remembers_whether_it_holds() {
        let gate = L1FeeGate::new(480, caps());
        let high_fees = L1Fees {
            blob_base_fee: U256::from(MAX_BLOB_BASE_FEE + 1),
            ..fees(Duration::ZERO)
        };

        assert!(!gate.is_holding());
        assert!(!gate.allows(Some(&high_fees), now()));
        assert!(gate.is_holding());
        assert!(gate.allows(Some(&fees(Duration::ZERO)), now()));
        assert!(!gate.is_holding());
    }

    #[test]
    fn oracle_selectors_match_the_predeploy() {
        assert_eq!(
            GasPriceOracle::l1BaseFeeCall::SELECTOR,
            hex_literal::hex!("519b4bd3")
        );
        assert_eq!(
            GasPriceOracle::blobBaseFeeCall::SELECTOR,
            hex_literal::hex!("f8206140")
        );
    }
}
