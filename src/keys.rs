use aws_config::BehaviorVersion;
use aws_sdk_kms::types::{KeySpec, KeyUsageType};
use ethers::core::k256::ecdsa::SigningKey;
use ethers::signers::Wallet;
use eyre::{Context, ContextCompat};
pub use universal_signer::UniversalSigner;

use crate::aws::ethers_signer::AwsSigner;
use crate::config::{KmsKeysConfig, LocalKeysConfig};

mod universal_signer;

#[async_trait::async_trait]
pub trait KeysSource: Send + Sync + 'static {
    /// Returns a key id and signer
    async fn new_signer(&self) -> eyre::Result<(String, UniversalSigner)>;

    /// Loads the key using the provided id
    async fn load_signer(&self, id: String) -> eyre::Result<UniversalSigner>;
}

pub struct KmsKeys {
    kms_client: aws_sdk_kms::Client,
}

impl KmsKeys {
    pub async fn new(_config: &KmsKeysConfig) -> eyre::Result<Self> {
        let aws_config =
            aws_config::load_defaults(BehaviorVersion::latest()).await;

        let kms_client = aws_sdk_kms::Client::new(&aws_config);

        Ok(Self { kms_client })
    }
}

#[async_trait::async_trait]
impl KeysSource for KmsKeys {
    async fn new_signer(&self) -> eyre::Result<(String, UniversalSigner)> {
        let kms_key = self
            .kms_client
            .create_key()
            .key_spec(KeySpec::EccSecgP256K1)
            .key_usage(KeyUsageType::SignVerify)
            .send()
            .await
            .context("AWS Error")?;

        let key_id =
            kms_key.key_metadata.context("Missing key metadata")?.key_id;

        let signer = AwsSigner::new(
            self.kms_client.clone(),
            key_id.clone(),
            1, // TODO: get chain id from provider
        )
        .await?;

        Ok((key_id, UniversalSigner::Aws(signer)))
    }

    async fn load_signer(&self, id: String) -> eyre::Result<UniversalSigner> {
        let signer = AwsSigner::new(
            self.kms_client.clone(),
            id.clone(),
            1, // TODO: get chain id from provider
        )
        .await?;

        Ok(UniversalSigner::Aws(signer))
    }
}

pub struct LocalKeys {
    rng: rand::rngs::OsRng,
}

impl LocalKeys {
    pub fn new(_config: &LocalKeysConfig) -> Self {
        Self {
            rng: rand::rngs::OsRng,
        }
    }
}

#[async_trait::async_trait]
impl KeysSource for LocalKeys {
    async fn new_signer(&self) -> eyre::Result<(String, UniversalSigner)> {
        let signing_key = SigningKey::random(&mut self.rng.clone());

        let key_id = signing_key.to_bytes().to_vec();
        let key_id = hex::encode(key_id);

        let signer = Wallet::from(signing_key);

        Ok((key_id, UniversalSigner::Local(signer)))
    }

    async fn load_signer(&self, id: String) -> eyre::Result<UniversalSigner> {
        let key_id = hex::decode(id)?;
        let signing_key = SigningKey::from_slice(key_id.as_slice())?;

        let signer = Wallet::from(signing_key);

        Ok(UniversalSigner::Local(signer))
    }
}

#[cfg(test)]
mod tests {
    use ethers::signers::Signer;

    use super::*;

    #[tokio::test]
    async fn local_roundtrip() -> eyre::Result<()> {
        let keys_source = LocalKeys::new(&LocalKeysConfig {});

        let (id, signer) = keys_source.new_signer().await?;

        let address = signer.address();

        let signer = keys_source.load_signer(id).await?;

        assert_eq!(address, signer.address());

        Ok(())
    }
}
