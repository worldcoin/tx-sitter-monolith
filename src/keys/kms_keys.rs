use std::time::Duration;

use alloy::network::TxSigner;
use alloy::signers::aws::AwsSigner;
use aws_config::{BehaviorVersion, SdkConfig};
use aws_sdk_kms::config::retry::RetryConfig;
use aws_sdk_kms::config::timeout::TimeoutConfig;
use aws_sdk_kms::types::{KeySpec, KeyUsageType, Tag};
use eyre::{Context, ContextCompat};
use tokio::time::timeout;

use super::{KeysSource, UniversalSigner};
use crate::config::KmsKeysConfig;

/// Max amount of time loading the AWS config may take, including region and
/// credential lookups through IMDS, ECS or STS.
const AWS_CONFIG_LOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// Max amount of time a single KMS request may take before the SDK retries
/// it. KMS usually answers within tens of milliseconds.
const KMS_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(2);

pub struct KmsKeys {
    kms_client: aws_sdk_kms::Client,
    kms_timeout: Duration,
}

impl KmsKeys {
    pub async fn new(config: &KmsKeysConfig) -> eyre::Result<Self> {
        tracing::info!("Initializing AWS client - loading config");
        let aws_config = timeout(
            AWS_CONFIG_LOAD_TIMEOUT,
            aws_config::load_defaults(BehaviorVersion::latest()),
        )
        .await
        .context("Timed out loading the AWS config")?;

        tracing::info!(?aws_config, "Initializing KMS client");
        let kms_keys = Self::from_aws_config(&aws_config, config);

        tracing::info!("Initialized KMS client");

        Ok(kms_keys)
    }

    /// Builds a KMS client that keeps the retry config resolved into
    /// `aws_config` and bounds every operation by `config.kms_timeout`.
    fn from_aws_config(aws_config: &SdkConfig, config: &KmsKeysConfig) -> Self {
        let kms_config = aws_sdk_kms::config::Builder::from(aws_config)
            .timeout_config(
                TimeoutConfig::builder()
                    .operation_timeout(config.kms_timeout)
                    .operation_attempt_timeout(KMS_ATTEMPT_TIMEOUT)
                    .build(),
            )
            .build();

        Self {
            kms_client: aws_sdk_kms::Client::from_conf(kms_config),
            kms_timeout: config.kms_timeout,
        }
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
            .customize()
            // CreateKey isn't idempotent: retrying after a slow attempt that
            // succeeded would leave that key without a RelayerName tag
            .config_override(
                aws_sdk_kms::config::Builder::default()
                    .retry_config(RetryConfig::disabled())
                    .timeout_config(
                        TimeoutConfig::builder()
                            .operation_attempt_timeout(self.kms_timeout)
                            .build(),
                    ),
            )
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

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use aws_sdk_kms::config::{Credentials, Region, SharedCredentialsProvider};

    use super::*;
    use crate::test_utils::UnresponsiveServer;

    #[tokio::test]
    async fn create_key_is_attempted_once() -> eyre::Result<()> {
        let kms = UnresponsiveServer::spawn().await?;
        let kms_timeout = Duration::from_secs(4);
        let keys = KmsKeys::from_aws_config(
            &aws_config(&kms, 3),
            &KmsKeysConfig { kms_timeout },
        );

        let started = Instant::now();
        let result = keys.new_signer("relayer").await;
        let elapsed = started.elapsed();

        assert!(result.is_err(), "Unexpected key: {result:?}");
        assert!(elapsed <= kms_timeout * 2, "Took {elapsed:?}");
        assert_eq!(kms.accepted_connections(), 1);

        Ok(())
    }

    #[tokio::test]
    async fn kms_requests_follow_the_resolved_retry_config() -> eyre::Result<()>
    {
        let kms = UnresponsiveServer::spawn().await?;
        let max_attempts = 2;
        let kms_timeout = Duration::from_secs(30);
        let keys = KmsKeys::from_aws_config(
            &aws_config(&kms, max_attempts),
            &KmsKeysConfig { kms_timeout },
        );

        let started = Instant::now();
        let result = keys.load_signer("key-id".to_string()).await;
        let elapsed = started.elapsed();

        assert!(result.is_err(), "Unexpected signer: {result:?}");
        assert!(elapsed < kms_timeout, "Took {elapsed:?}");
        assert_eq!(kms.accepted_connections(), max_attempts as usize);

        Ok(())
    }

    #[tokio::test]
    async fn kms_operations_fail_within_kms_timeout() -> eyre::Result<()> {
        let kms = UnresponsiveServer::spawn().await?;
        let kms_timeout = Duration::from_secs(3);
        let keys = KmsKeys::from_aws_config(
            &aws_config(&kms, 10),
            &KmsKeysConfig { kms_timeout },
        );

        let started = Instant::now();
        let result = keys.load_signer("key-id".to_string()).await;
        let elapsed = started.elapsed();

        assert!(result.is_err(), "Unexpected signer: {result:?}");
        assert!(elapsed >= kms_timeout, "Took {elapsed:?}");
        assert!(elapsed <= kms_timeout * 2, "Took {elapsed:?}");

        Ok(())
    }

    /// The config `aws_config::load_defaults` resolves for a KMS endpoint at
    /// `kms` when `AWS_MAX_ATTEMPTS` is `max_attempts`.
    fn aws_config(kms: &UnresponsiveServer, max_attempts: u32) -> SdkConfig {
        SdkConfig::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .endpoint_url(format!("http://{}", kms.addr))
            .credentials_provider(SharedCredentialsProvider::new(
                Credentials::new("key-id", "secret", None, None, "test"),
            ))
            .retry_config(
                RetryConfig::standard().with_max_attempts(max_attempts),
            )
            .build()
    }
}
