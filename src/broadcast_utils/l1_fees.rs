use std::sync::{Mutex, MutexGuard, PoisonError};

use alloy::eips::BlockNumberOrTag;
use alloy::primitives::{address, Address, U256};
use alloy::providers::{Provider, RootProvider};
use alloy::rpc::types::TransactionRequest;
use alloy::sol;
use alloy::sol_types::SolCall;
use chrono::{DateTime, Utc};
use eyre::Context;

use crate::config::OpStackConfig;
use crate::failover_transport::describe_error;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct L1Fees {
    pub l1_base_fee: U256,
    pub blob_base_fee: U256,
    /// When the read of the fees from the oracle started
    pub read_at: DateTime<Utc>,
}

/// Reads the L1 fees of the chain behind `rpc` from its `GasPriceOracle`
/// predeploy, at the latest block.
///
/// The fees are stamped with the time the read started. It fails if either
/// call fails, or returns something other than a `uint256`, for example on a
/// chain without the predeploy. Transport errors are described with
/// [`describe_error`], so the error is safe to log.
pub async fn read_l1_fees(rpc: &RootProvider) -> eyre::Result<L1Fees> {
    let read_at = Utc::now();

    let (l1_base_fee, blob_base_fee) = tokio::try_join!(
        call_oracle(rpc, GasPriceOracle::l1BaseFeeCall {}),
        call_oracle(rpc, GasPriceOracle::blobBaseFeeCall {}),
    )?;

    Ok(L1Fees {
        l1_base_fee,
        blob_base_fee,
        read_at,
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
        .map_err(|err| {
            eyre::eyre!(
                "Calling GasPriceOracle.{}: {}",
                C::SIGNATURE,
                describe_error(&err)
            )
        })?;

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

impl L1FeeHold {
    const ALL: [Self; 3] = [
        Self::Unknown,
        Self::L1BaseFeeAboveCap,
        Self::BlobBaseFeeAboveCap,
    ];

    /// The `reason` tag of the `l1_fee_gate_holding` gauge
    fn reason(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::L1BaseFeeAboveCap => "l1_base_fee",
            Self::BlobBaseFeeAboveCap => "blob_base_fee",
        }
    }
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
/// It keeps the L1 fees read last, which the worker's L1 fee task records,
/// and every [`L1FeeGate::check`] decides from them whether the chain's
/// transactions are held back. It remembers the last decision, so that it
/// logs at warn only when that changes rather than on every check.
pub struct L1FeeGate {
    chain_id: u64,
    caps: OpStackConfig,
    state: Mutex<GateState>,
}

struct GateState {
    /// The L1 fees read last
    fees: Option<L1Fees>,
    /// Why the gate held back transactions at its last check
    hold: Option<L1FeeHold>,
}

impl L1FeeGate {
    /// Creates the gate of `chain_id` with the L1 fees read last, for
    /// example by an earlier process.
    ///
    /// Its first check logs if it holds back transactions.
    pub fn new(
        chain_id: u64,
        caps: OpStackConfig,
        fees: Option<L1Fees>,
    ) -> Self {
        Self {
            chain_id,
            caps,
            state: Mutex::new(GateState { fees, hold: None }),
        }
    }

    /// Records `fees` as the L1 fees read last.
    pub fn record(&self, fees: L1Fees) {
        self.lock_state().fees = Some(fees);
    }

    /// Returns the L1 fees read last.
    pub fn last_fees(&self) -> Option<L1Fees> {
        self.lock_state().fees
    }

    /// Returns why the chain's transactions are held back now, or `None` if
    /// they may go out, see [`l1_fee_hold`].
    ///
    /// It sets the `l1_fee_gate_holding` gauge, 1 for the reason it holds and
    /// 0 for the others, and the `l1_fees_age_seconds` gauge, and logs at
    /// warn when it starts or stops holding, or holds for another reason.
    pub fn check(&self) -> Option<L1FeeHold> {
        let now = Utc::now();
        let (fees, hold, previous_hold) = {
            let mut state = self.lock_state();
            let hold = l1_fee_hold(&self.caps, state.fees.as_ref(), now);
            let previous_hold = std::mem::replace(&mut state.hold, hold);

            (state.fees, hold, previous_hold)
        };

        let chain_id = self.chain_id.to_string();
        for reason in L1FeeHold::ALL {
            let is_holding = hold == Some(reason);
            metrics::gauge!(
                "l1_fee_gate_holding",
                &[
                    ("chain_id", chain_id.clone()),
                    ("reason", reason.reason().to_string())
                ]
            )
            .set(if is_holding { 1.0 } else { 0.0 });
        }
        if let Some(fees) = fees {
            let age = now - fees.read_at;
            metrics::gauge!("l1_fees_age_seconds", &[("chain_id", chain_id)])
                .set(age.num_milliseconds() as f64 / 1000.0);
        }

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

        hold
    }

    fn lock_state(&self) -> MutexGuard<'_, GateState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use base_api_types::DecimalU256;
    use chrono::TimeZone;
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};
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
    fn gate_checks_the_fees_it_recorded_last() {
        let fresh_fees = L1Fees {
            read_at: Utc::now(),
            ..fees(Duration::ZERO)
        };
        let gate = L1FeeGate::new(480, caps(), None);

        assert_eq!(gate.check(), Some(L1FeeHold::Unknown));

        gate.record(L1Fees {
            blob_base_fee: U256::from(MAX_BLOB_BASE_FEE + 1),
            ..fresh_fees
        });
        assert_eq!(gate.check(), Some(L1FeeHold::BlobBaseFeeAboveCap));

        gate.record(fresh_fees);
        assert_eq!(gate.check(), None);
        assert_eq!(gate.last_fees(), Some(fresh_fees));
    }

    #[test]
    fn gate_starts_from_the_fees_it_is_given() {
        let fresh_fees = L1Fees {
            read_at: Utc::now(),
            ..fees(Duration::ZERO)
        };

        assert_eq!(L1FeeGate::new(480, caps(), Some(fresh_fees)).check(), None);
    }

    #[test_case(None, Some("unknown") ; "fees not read")]
    #[test_case(Some(MAX_L1_BASE_FEE + 1), Some("l1_base_fee") ; "l1 base fee above its cap")]
    #[test_case(Some(MAX_L1_BASE_FEE), None ; "l1 base fee at its cap")]
    fn check_reports_why_it_holds(
        l1_base_fee: Option<u64>,
        holding_reason: Option<&str>,
    ) {
        let read_at = Utc::now() - chrono::Duration::seconds(3);
        let fees = l1_base_fee.map(|l1_base_fee| L1Fees {
            l1_base_fee: U256::from(l1_base_fee),
            read_at,
            ..fees(Duration::ZERO)
        });
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();

        metrics::with_local_recorder(&recorder, || {
            L1FeeGate::new(480, caps(), fees).check()
        });

        let gauges: HashMap<_, _> = snapshotter
            .snapshot()
            .into_vec()
            .into_iter()
            .filter_map(|(key, _, _, value)| {
                let DebugValue::Gauge(value) = value else {
                    return None;
                };
                let labels: Vec<_> = key
                    .key()
                    .labels()
                    .map(|label| format!("{}={}", label.key(), label.value()))
                    .collect();

                Some((
                    format!("{}{{{}}}", key.key().name(), labels.join(",")),
                    value.into_inner(),
                ))
            })
            .collect();

        for reason in ["l1_base_fee", "blob_base_fee", "unknown"] {
            let expected = if holding_reason == Some(reason) {
                1.0
            } else {
                0.0
            };
            assert_eq!(
                gauges[&format!(
                    "l1_fee_gate_holding{{chain_id=480,reason={reason}}}"
                )],
                expected,
                "{reason}"
            );
        }
        let age = gauges.get("l1_fees_age_seconds{chain_id=480}");
        match fees {
            Some(_) => assert!(age.is_some_and(|age| *age >= 3.0), "{age:?}"),
            None => assert_eq!(age, None),
        }
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
