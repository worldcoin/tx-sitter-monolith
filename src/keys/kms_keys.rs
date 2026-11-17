use std::time::Duration;

use alloy::signers::aws::{AwsSigner, AwsSignerError};
use aws_config::{BehaviorVersion, SdkConfig};
use aws_sdk_kms::config::timeout::TimeoutConfig;
use aws_sdk_kms::error::ProvideErrorMetadata;
use aws_smithy_runtime_api::client::result::SdkError;
use eyre::Context;
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
        }
    }
}

#[async_trait::async_trait]
impl KeysSource for KmsKeys {
    async fn load_signer(&self, id: String) -> eyre::Result<UniversalSigner> {
        let signer = AwsSigner::new(self.kms_client.clone(), id, None).await?;

        Ok(UniversalSigner::Aws(signer))
    }
}

/// Returns whether `err`, from loading a KMS key, may pass when retried: the
/// request timed out or didn't get a response, or KMS answered with a
/// throttling, internal or availability error.
pub fn is_transient_error(err: &eyre::Report) -> bool {
    let Some(AwsSignerError::GetPublicKey(err)) = err.downcast_ref() else {
        return false;
    };

    match err {
        SdkError::TimeoutError(_)
        | SdkError::DispatchFailure(_)
        | SdkError::ResponseError(_) => true,
        SdkError::ServiceError(service_err) => {
            let status = service_err.raw().status().as_u16();
            let err = service_err.err();

            err.is_dependency_timeout_exception()
                || err.is_key_unavailable_exception()
                || err.is_kms_internal_exception()
                || err.code() == Some("ThrottlingException")
                || status == 429
                || status >= 500
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use aws_sdk_kms::config::retry::RetryConfig;
    use aws_sdk_kms::config::{Credentials, Region, SharedCredentialsProvider};
    use poem::http::StatusCode;
    use poem::listener::{Acceptor, Listener};
    use test_case::test_case;

    use super::*;
    use crate::test_utils::UnresponsiveServer;

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

    #[tokio::test]
    async fn timeouts_are_transient() -> eyre::Result<()> {
        let kms = UnresponsiveServer::spawn().await?;
        let keys = KmsKeys::from_aws_config(
            &aws_config(&kms, 1),
            &KmsKeysConfig {
                kms_timeout: Duration::from_secs(1),
            },
        );

        let err = keys.load_signer("key-id".to_string()).await.unwrap_err();

        assert!(is_transient_error(&err), "{err:?}");

        Ok(())
    }

    #[test_case(StatusCode::BAD_REQUEST, "NotFoundException", false ; "missing key")]
    #[test_case(StatusCode::BAD_REQUEST, "AccessDeniedException", false ; "missing permission")]
    #[test_case(StatusCode::BAD_REQUEST, "ThrottlingException", true ; "throttling")]
    #[test_case(StatusCode::INTERNAL_SERVER_ERROR, "KMSInternalException", true ; "internal error")]
    #[tokio::test]
    async fn classifies_kms_errors(
        status: StatusCode,
        error_type: &'static str,
        is_transient: bool,
    ) -> eyre::Result<()> {
        let kms = spawn_failing_kms(status, error_type).await?;
        let keys = KmsKeys::from_aws_config(
            &aws_config_for(&kms, 1),
            &KmsKeysConfig {
                kms_timeout: Duration::from_secs(5),
            },
        );

        let err = keys.load_signer("key-id".to_string()).await.unwrap_err();

        assert_eq!(is_transient_error(&err), is_transient, "{err:?}");

        Ok(())
    }

    /// Starts a KMS endpoint that answers every request with `status` and an
    /// error of type `error_type`, and returns its URL.
    async fn spawn_failing_kms(
        status: StatusCode,
        error_type: &'static str,
    ) -> eyre::Result<String> {
        let endpoint = poem::endpoint::make_sync(move |_| {
            poem::Response::builder()
                .status(status)
                .content_type("application/x-amz-json-1.1")
                .body(format!(
                    r#"{{"__type":"{error_type}","message":"stub"}}"#
                ))
        });
        let acceptor = poem::listener::TcpListener::bind("127.0.0.1:0")
            .into_acceptor()
            .await?;
        let addr = acceptor.local_addr()[0]
            .as_socket_addr()
            .copied()
            .ok_or_else(|| eyre::eyre!("Stub KMS has no socket address"))?;
        tokio::spawn(poem::Server::new_with_acceptor(acceptor).run(endpoint));

        Ok(format!("http://{addr}"))
    }

    fn aws_config(kms: &UnresponsiveServer, max_attempts: u32) -> SdkConfig {
        aws_config_for(&format!("http://{}", kms.addr), max_attempts)
    }

    /// The config `aws_config::load_defaults` resolves for a KMS endpoint at
    /// `endpoint` when `AWS_MAX_ATTEMPTS` is `max_attempts`.
    fn aws_config_for(endpoint: &str, max_attempts: u32) -> SdkConfig {
        SdkConfig::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .endpoint_url(endpoint)
            .credentials_provider(SharedCredentialsProvider::new(
                Credentials::new("key-id", "secret", None, None, "test"),
            ))
            .retry_config(
                RetryConfig::standard().with_max_attempts(max_attempts),
            )
            .build()
    }
}
