use std::time::Duration;

use alloy::network::TransactionBuilder;
use alloy::node_bindings::{Anvil, AnvilInstance};
use alloy::primitives::U256;
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;

use super::prelude::{
    DEFAULT_ANVIL_ACCOUNT, DEFAULT_ANVIL_BLOCK_TIME,
    SECONDARY_ANVIL_PRIVATE_KEY,
};

// tx-sitter estimates tips from the rewards of recent blocks. With a zero tip
// every estimate is zero and an escalated transaction is identical to the
// original one, so the seed transaction pays a tip (in wei).
const SEED_TX_MAX_PRIORITY_FEE_PER_GAS: u128 = 3_000_000_000;
const SEED_TX_MAX_FEE_PER_GAS: u128 = 10_000_000_000;

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

        let anvil = anvil.try_spawn()?;

        let signer = PrivateKeySigner::from_slice(SECONDARY_ANVIL_PRIVATE_KEY)?;
        let provider = ProviderBuilder::new()
            .wallet(signer)
            .connect_http(anvil.endpoint_url());

        // Wait for the chain to start and produce at least one block
        tokio::time::sleep(Duration::from_secs(block_time)).await;

        // We need to seed some transactions so we can get fee estimates on the first block
        provider
            .send_transaction(
                TransactionRequest::default()
                    .with_to(DEFAULT_ANVIL_ACCOUNT)
                    .with_value(U256::from(100u64))
                    .with_max_priority_fee_per_gas(
                        SEED_TX_MAX_PRIORITY_FEE_PER_GAS,
                    )
                    .with_max_fee_per_gas(SEED_TX_MAX_FEE_PER_GAS),
            )
            .await?
            .get_receipt()
            .await?;

        Ok(anvil)
    }
}
