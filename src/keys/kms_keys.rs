use alloy::network::TxSigner;
use alloy::signers::aws::AwsSigner;
use aws_config::BehaviorVersion;
use aws_sdk_kms::config::retry::RetryConfig;
use aws_sdk_kms::config::timeout::TimeoutConfig;
use aws_sdk_kms::types::{KeySpec, KeyUsageType, Tag};
use eyre::{Context, ContextCompat};

use super::{KeysSource, UniversalSigner};
use crate::config::KmsKeysConfig;

/// Attempts per KMS operation, the first one included.
const KMS_MAX_ATTEMPTS: u32 = 3;

pub struct KmsKeys {
    kms_client: aws_sdk_kms::Client,
}

impl KmsKeys {
    pub async fn new(config: &KmsKeysConfig) -> eyre::Result<Self> {
        tracing::info!("Initializing AWS client - loading config");
        let aws_config =
            aws_config::load_defaults(BehaviorVersion::latest()).await;

        tracing::info!(?aws_config, "Initializing KMS client");
        let kms_config = aws_sdk_kms::config::Builder::from(&aws_config)
            .retry_config(
                RetryConfig::standard().with_max_attempts(KMS_MAX_ATTEMPTS),
            )
            .timeout_config(
                TimeoutConfig::builder()
                    .operation_timeout(config.kms_timeout)
                    .operation_attempt_timeout(
                        config.kms_timeout / KMS_MAX_ATTEMPTS,
                    )
                    .build(),
            )
            .build();
        let kms_client = aws_sdk_kms::Client::from_conf(kms_config);

        tracing::info!("Initialized KMS client");

        Ok(Self { kms_client })
    }
}

#[async_trait::async_trait]
impl KeysSource for KmsKeys {
    async fn new_signer(
        &self,
        meta_name: &str,
    ) -> eyre::Result<(String, UniversalSigner)> {
        let kms_key = self
            .kms_client
            .create_key()
            .key_spec(KeySpec::EccSecgP256K1)
            .key_usage(KeyUsageType::SignVerify)
            .tags(
                Tag::builder()
                    .tag_key("CreatedBy")
                    .tag_value("tx-sitter")
                    .build()?,
            )
            .description(format!("Key of relayer {meta_name}"))
            .send()
            .await
            .context("AWS Error")?;

        let key_id =
            kms_key.key_metadata.context("Missing key metadata")?.key_id;

        let signer =
            AwsSigner::new(self.kms_client.clone(), key_id.clone(), None)
                .await?;

        let address = signer.address();

        self.kms_client
            .update_alias()
            .target_key_id(key_id.clone())
            .alias_name(format!("{meta_name}-{address:?}"));

        self.kms_client
            .tag_resource()
            .key_id(key_id.clone())
            .tags(
                Tag::builder()
                    .tag_key("RelayerName")
                    .tag_value(meta_name)
                    .build()?,
            )
            .tags(
                Tag::builder()
                    .tag_key("RelayerAddress")
                    .tag_value(format!("{address:?}"))
                    .build()?,
            )
            .send()
            .await?;

        Ok((key_id, UniversalSigner::Aws(signer)))
    }

    async fn load_signer(&self, id: String) -> eyre::Result<UniversalSigner> {
        let signer = AwsSigner::new(self.kms_client.clone(), id, None).await?;

        Ok(UniversalSigner::Aws(signer))
    }
}
