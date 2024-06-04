#[cfg(test)]
mod tests {
    use alloy::consensus::{SidecarBuilder, SimpleCoder};
    use alloy::eips::eip4844::DATA_GAS_PER_BLOB;
    use alloy::network::TransactionBuilder;
    use alloy::node_bindings::Anvil;
    use alloy::providers::{Provider, ProviderBuilder};
    use alloy::rpc::types::eth::TransactionRequest;

    #[tokio::test]
    async fn send_4844() -> eyre::Result<()> {
        let anvil = Anvil::new().args(["--hardfork", "latest"]).try_spawn()?;

        let endpoint = anvil.endpoint().parse()?;
        let provider = ProviderBuilder::new()
            .with_recommended_fillers()
            .on_http(endpoint);
        let alice = anvil.addresses()[0];
        let bob = anvil.addresses()[1];

        // The actual max blob size is 131072, but SimpleEncoder sacrafices ~3% of the space for encoding
        let estimated_max_blob_size = 125829;
        // ~3813 idcomms
        let large_data = vec![1u8; estimated_max_blob_size];
        let sidecar: SidecarBuilder<SimpleCoder> =
            SidecarBuilder::from_slice(&large_data);

        let sidecar = sidecar.build()?;

        let gas_price = provider.get_gas_price().await?;
        let eip1559_est = provider.estimate_eip1559_fees(None).await?;
        let mut tx = TransactionRequest::default()
            .with_from(alice)
            .with_to(bob)
            .with_nonce(0)
            .with_max_fee_per_gas(eip1559_est.max_fee_per_gas)
            .with_max_priority_fee_per_gas(eip1559_est.max_priority_fee_per_gas)
            .with_max_fee_per_blob_gas(gas_price)
            .with_blob_sidecar(sidecar);

        tx.populate_blob_hashes();

        // let wallet: LocalWallet = anvil.keys()[0].clone().into();
        // let signer: EthereumSigner = wallet.with_chain_id(Some(1)).into();
        // let tx_envelope = tx.build(&signer).await?;
        // let tx_encoded = tx_envelope.encoded_2718();

        // Send the raw transaction and retrieve the transaction receipt.
        // let receipt = provider
        //     .send_raw_transaction(&tx_encoded)
        //     .await?
        //     .get_receipt()
        //     .await?;

        let pending_tx = provider.send_transaction(tx).await?;
        let tx_hash = pending_tx.tx_hash().clone();
        println!("Pending transaction... {}", tx_hash);
        let receipt = &pending_tx.get_receipt().await?;

        println!(
            "Transaction included in block {}",
            receipt.block_number.expect("Failed to get block number")
        );

        println!("Receipt: {:?}", receipt);

        let block = provider
            .get_block_by_hash(receipt.block_hash.unwrap(), true)
            .await?;
        println!("Block: {:?}", block);

        assert_eq!(receipt.from, alice);
        assert_eq!(receipt.to, Some(bob));
        assert_eq!(receipt.blob_gas_used.unwrap(), DATA_GAS_PER_BLOB as u128);

        Ok(())
    }
}
