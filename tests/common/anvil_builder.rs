use std::time::Duration;

use alloy::consensus::{SidecarBuilder, SimpleCoder};
use alloy::network::TransactionBuilder;
use alloy::primitives::U256 as AlloyU256;
use alloy::providers::Provider;
use alloy::rpc::types::eth::TransactionRequest;
use ethers::providers::Middleware;
use ethers::types::{Eip1559TransactionRequest, U256};
use ethers::utils::{Anvil, AnvilInstance};

use alloy::node_bindings::{
    Anvil as AlloyAnvil, AnvilInstance as AlloyAnvilInstance,
};
use tracing::info;

use crate::{_setup_middleware, DEFAULT_ANVIL_PRIVATE_KEY};

use super::prelude::{
    setup_middleware, DEFAULT_ANVIL_ACCOUNT, DEFAULT_ANVIL_BLOCK_TIME,
    SECONDARY_ANVIL_PRIVATE_KEY,
};

#[derive(Debug, Clone, Default)]
pub struct AnvilBuilder {
    pub block_time: Option<u64>,
    pub port: Option<u16>,
}

impl AnvilBuilder {
    pub fn block_time(mut self, block_time: u64) -> Self {
        self.block_time = Some(block_time);
        self
    }

    pub fn port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    pub async fn spawn(self) -> eyre::Result<AnvilInstance> {
        let mut anvil = Anvil::new();

        let block_time = if let Some(block_time) = self.block_time {
            block_time
        } else {
            DEFAULT_ANVIL_BLOCK_TIME
        };
        anvil = anvil.block_time(block_time);

        if let Some(port) = self.port {
            anvil = anvil.port(port);
        }

        let anvil = anvil.spawn();

        let middleware =
            setup_middleware(anvil.endpoint(), SECONDARY_ANVIL_PRIVATE_KEY)
                .await?;

        // Wait for the chain to start and produce at least one block
        tokio::time::sleep(Duration::from_secs(block_time)).await;

        // We need to seed some transactions so we can get fee estimates on the first block
        middleware
            .send_transaction(
                Eip1559TransactionRequest {
                    to: Some(DEFAULT_ANVIL_ACCOUNT.into()),
                    value: Some(U256::from(100u64)),
                    ..Default::default()
                },
                None,
            )
            .await?
            .await?;

        Ok(anvil)
    }

    pub async fn _spawn(self) -> eyre::Result<AlloyAnvilInstance> {
        let mut anvil = AlloyAnvil::new();

        let block_time = if let Some(block_time) = self.block_time {
            block_time
        } else {
            DEFAULT_ANVIL_BLOCK_TIME
        };
        anvil = anvil.block_time(block_time);

        if let Some(port) = self.port {
            anvil = anvil.port(port);
        }

        let anvil = anvil.try_spawn()?;

        let middleware =
            _setup_middleware(&anvil.endpoint(), DEFAULT_ANVIL_PRIVATE_KEY)
                .await?;

        // Wait for the chain to start and produce at least one block
        tokio::time::sleep(Duration::from_secs(block_time)).await;

        let sidecar: SidecarBuilder<SimpleCoder> =
            SidecarBuilder::from_slice(&vec![1u8; 1000]);
        let sidecar = sidecar.build()?;
        let gas_price = middleware.get_gas_price().await?;
        let eip1559_est = middleware.estimate_eip1559_fees(None).await?;

        let base_fee =
            eip1559_est.max_fee_per_gas - eip1559_est.max_priority_fee_per_gas;
        let priority_fee = 10000000u128;

        let mut tx: TransactionRequest = TransactionRequest::default()
            .with_from(DEFAULT_ANVIL_ACCOUNT.to_fixed_bytes().into())
            .with_to(DEFAULT_ANVIL_ACCOUNT.to_fixed_bytes().into())
            .with_value(AlloyU256::from(100u64))
            .with_max_fee_per_gas(priority_fee + base_fee)
            .with_max_priority_fee_per_gas(priority_fee)
            .with_max_fee_per_blob_gas(gas_price)
            .with_blob_sidecar(sidecar);

        tx.populate_blob_hashes();

        // We need to seed some transactions so we can get fee estimates on the first block
        let receipt =
            middleware.send_transaction(tx).await?.get_receipt().await?;

        info!("========================== Receipt: {:?}", receipt);

        Ok(anvil)
    }
}
