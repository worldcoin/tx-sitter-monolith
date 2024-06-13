use alloy::consensus::{SidecarBuilder, SimpleCoder};
use alloy::network::TransactionBuilder;
use alloy::primitives::Address;
use alloy::rpc::types::eth::TransactionRequest;
use ethers::types::U256;
use eyre::ContextCompat;

use self::gas_estimation::FeesEstimate;
use crate::app::App;
use crate::db::data::{AddressWrapper, U256Wrapper};
use crate::db::{TxForEscalation, UnsentTx};
use crate::types::RelayerInfo;

pub mod gas_estimation;

/// Returns a tuple of max and max priority fee per gas
pub fn calculate_gas_fees_from_estimates(
    estimates: &FeesEstimate,
    tx_priority_index: usize,
    max_base_fee_per_gas: U256,
    max_base_fee_per_blob_gas: u128,
) -> (U256, U256, U256) {
    let max_priority_fee_per_gas = estimates.percentile_fees[tx_priority_index];

    let max_fee_per_gas = max_base_fee_per_gas + max_priority_fee_per_gas;

    let max_fee_per_blob_gas =
        U256::from(max_base_fee_per_blob_gas) + max_priority_fee_per_gas;

    (
        max_fee_per_gas,
        max_priority_fee_per_gas,
        max_fee_per_blob_gas,
    )
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

pub async fn create_transaction_request<T: ToTransactionRequest>(
    tx: &T,
    signer_address: Address,
    max_fee_per_gas: U256,
    max_priority_fee_per_gas: U256,
    max_fee_per_blob_gas: u128,
) -> eyre::Result<TransactionRequest> {
    let request = tx
        .to_transaction_request(
            signer_address,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            max_fee_per_blob_gas,
        )
        .await?;
    Ok(request)
}

#[allow(clippy::too_many_arguments)]
async fn create_tx_request(
    to: AddressWrapper,
    gas_limit: U256Wrapper,
    value: U256Wrapper,
    data: Vec<u8>,
    nonce: u64,
    chain_id: u64,
    blobs: Option<Vec<Vec<u8>>>,
    signer_address: Address,
    max_fee_per_gas: U256,
    max_priority_fee_per_gas: U256,
    max_fee_per_blob_gas: u128,
) -> eyre::Result<TransactionRequest> {
    let to_alloy = to.0.to_fixed_bytes();
    let data: alloy::primitives::Bytes = data.to_vec().into();
    let mut alloy_value = [0_u8; 32];
    value.0.to_little_endian(&mut alloy_value);

    let mut tx_request = TransactionRequest::default()
        .with_from(signer_address)
        .with_to(alloy::primitives::Address::from_slice(&to_alloy))
        .with_gas_limit(gas_limit.0.low_u128())
        .with_value(alloy::primitives::U256::from_le_slice(&alloy_value))
        .with_input(data)
        .with_nonce(nonce)
        .with_access_list(alloy::eips::eip2930::AccessList::default())
        .with_max_priority_fee_per_gas(max_priority_fee_per_gas.low_u128())
        .with_max_fee_per_gas(max_fee_per_gas.low_u128())
        .with_chain_id(chain_id);

    if let Some(blobs) = &blobs {
        let sidecar: SidecarBuilder<SimpleCoder> =
            SidecarBuilder::from_slice(&blobs[0]);

        let sidecar = sidecar.build()?;
        tx_request = tx_request
            .with_max_fee_per_blob_gas(max_fee_per_blob_gas)
            .with_blob_sidecar(sidecar);

        tx_request.populate_blob_hashes();
    }

    Ok(tx_request)
}

pub trait ToTransactionRequest {
    fn to_transaction_request(
        &self,
        signer_address: Address,
        max_fee_per_gas: U256,
        max_base_fee_per_gas: U256,
        max_fee_per_blob_gas: u128,
    ) -> impl std::future::Future<Output = eyre::Result<TransactionRequest>> + Send;
}

impl ToTransactionRequest for UnsentTx {
    async fn to_transaction_request(
        &self,
        signer_address: Address,
        max_fee_per_gas: U256,
        max_priority_fee_per_gas: U256,
        max_fee_per_blob_gas: u128,
    ) -> eyre::Result<TransactionRequest> {
        let request = create_tx_request(
            self.tx_to,
            self.gas_limit,
            self.value,
            self.data.clone(),
            self.nonce,
            self.chain_id,
            self.blobs.clone(),
            signer_address,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            max_fee_per_blob_gas,
        )
        .await?;
        Ok(request)
    }
}

impl ToTransactionRequest for TxForEscalation {
    async fn to_transaction_request(
        &self,
        signer_address: Address,
        max_fee_per_gas: U256,
        max_priority_fee_per_gas: U256,
        max_fee_per_blob_gas: u128,
    ) -> eyre::Result<TransactionRequest> {
        let request = create_tx_request(
            self.tx_to,
            self.gas_limit,
            self.value,
            self.data.clone(),
            self.nonce,
            self.chain_id,
            self.blobs.clone(),
            signer_address,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            max_fee_per_blob_gas,
        )
        .await?;
        Ok(request)
    }
}
