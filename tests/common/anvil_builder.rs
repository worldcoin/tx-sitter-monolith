use std::time::Duration;

use ethers::providers::Middleware;
use ethers::types::{Eip1559TransactionRequest, U256};
use ethers::utils::{Anvil, AnvilInstance};

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
}
