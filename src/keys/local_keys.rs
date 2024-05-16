use ethers::core::k256::ecdsa::SigningKey;
use ethers::signers::Wallet;

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

        let signer = Wallet::from(signing_key);

        Ok((key_id, UniversalSigner::Local(signer)))
    }

    async fn load_signer(&self, id: String) -> eyre::Result<UniversalSigner> {
        let signing_key = signing_key_from_hex(&id)?;

        let signer = Wallet::from(signing_key);

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
    use ethers::signers::Signer;

    use super::*;

    #[tokio::test]
    async fn local_roundtrip() -> eyre::Result<()> {
        let keys_source = LocalKeys::new(&LocalKeysConfig::default());

        let (id, signer) = keys_source.new_signer("meta name").await?;

        let address = signer.address();

        let signer = keys_source.load_signer(id).await?;

        assert_eq!(address, signer.address());

        Ok(())
    }
}
