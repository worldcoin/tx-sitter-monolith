use aws_config::BehaviorVersion;
use aws_sdk_kms::types::{KeySpec, KeyUsageType};
use eyre::{Context, ContextCompat};

use super::{KeysSource, UniversalSigner};
use crate::aws::ethers_signer::AwsSigner;
use crate::config::KmsKeysConfig;

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
