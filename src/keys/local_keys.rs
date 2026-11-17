use alloy::signers::k256::ecdsa::SigningKey;
use alloy::signers::local::PrivateKeySigner;

use super::universal_signer::UniversalSigner;
use super::KeysSource;
use crate::config::LocalKeysConfig;

pub struct LocalKeys {
    rng: rand::rngs::OsRng,
}

impl LocalKeys {
    pub fn new(_config: &LocalKeysConfig) -> Self {
        tracing::info!("Initializing local keys source");

        Self {
            rng: rand::rngs::OsRng,
        }
    }
}

#[async_trait::async_trait]
impl KeysSource for LocalKeys {
    async fn new_signer(
        &self,
        _meta_name: &str,
    ) -> eyre::Result<(String, UniversalSigner)> {
        let signing_key = SigningKey::random(&mut self.rng.clone());

        let key_id = signing_key.to_bytes().to_vec();
        let key_id = hex::encode(key_id);

        let signer = PrivateKeySigner::from(signing_key);

        Ok((key_id, UniversalSigner::Local(signer)))
    }

    async fn load_signer(&self, id: String) -> eyre::Result<UniversalSigner> {
        let signing_key = signing_key_from_hex(&id)?;

        let signer = PrivateKeySigner::from(signing_key);

        Ok(UniversalSigner::Local(signer))
    }
}

pub fn signing_key_from_hex(s: &str) -> eyre::Result<SigningKey> {
    let key_id = hex::decode(s)?;
    let signing_key = SigningKey::from_slice(key_id.as_slice())?;

    Ok(signing_key)
}

#[cfg(test)]
mod tests {
    use alloy::consensus::TxEip1559;
    use alloy::primitives::{address, bytes, keccak256, TxKind, U256};

    use super::*;

    /// One of anvil's default dev accounts.
    const PRIVATE_KEY_HEX: &str =
        "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    const SIGNED_TX_HEX: &str = concat!(
        "02f87a8201e0078459682f008506fc23ac00830186a0941ed53d680b8890dae2a63f",
        "673a85ffde1fd5c7a2880de0b6b3a764000084deadbeefc001a04c7bd71f526005a1",
        "fe6f5c451c6d949e315ec445e33f282569247667d9080e55a025383b2fb57a6fb56c",
        "7fb5db96c3296958a8a77169907382baa79ff2f4b0e45d"
    );
    const TX_HASH_HEX: &str =
        "bf56bb679b7cacdc7cb910594d3f4775d47ffff7984a1876f1789cd6014209b1";

    #[tokio::test]
    async fn local_roundtrip() -> eyre::Result<()> {
        let keys_source = LocalKeys::new(&LocalKeysConfig::default());

        let (id, signer) = keys_source.new_signer("meta name").await?;

        let address = signer.address();

        let signer = keys_source.load_signer(id).await?;

        assert_eq!(address, signer.address());

        Ok(())
    }

    /// Pins the bytes tx-sitter broadcasts, so a change in the signing stack
    /// can't silently alter the encoding, the signature or the hash.
    #[tokio::test]
    async fn local_signer_signs_pinned_eip1559_tx() -> eyre::Result<()> {
        let keys_source = LocalKeys::new(&LocalKeysConfig::default());
        let signer = keys_source.load_signer(PRIVATE_KEY_HEX.into()).await?;

        let tx = TxEip1559 {
            chain_id: 480,
            nonce: 7,
            gas_limit: 100_000,
            max_fee_per_gas: 30_000_000_000,
            max_priority_fee_per_gas: 1_500_000_000,
            to: TxKind::Call(address!(
                "1ed53d680b8890dae2a63f673a85ffde1fd5c7a2"
            )),
            value: U256::from(1_000_000_000_000_000_000u64),
            access_list: Default::default(),
            input: bytes!("deadbeef"),
        };

        let raw_signed_tx = signer.raw_signed_tx(tx).await?;

        assert_eq!(hex::encode(&raw_signed_tx), SIGNED_TX_HEX);
        assert_eq!(hex::encode(keccak256(&raw_signed_tx)), TX_HASH_HEX);

        Ok(())
    }
}
