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
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use alloy::consensus::{SignableTransaction, TxEip1559, TxEnvelope};
    use alloy::eips::eip2718::Decodable2718;
    use alloy::primitives::{Address, TxKind, U256};
    use alloy::signers::k256::ecdsa::signature::hazmat::PrehashSigner;
    use alloy::signers::k256::ecdsa::{Signature, SigningKey};
    use aws_sdk_kms::config::retry::RetryConfig;
    use aws_sdk_kms::config::{Credentials, Region, SharedCredentialsProvider};
    use base64::Engine;
    use eyre::ContextCompat;
    use poem::http::StatusCode;
    use poem::listener::{Acceptor, Listener};
    use serde_json::{json, Value};
    use test_case::test_case;

    use super::*;
    use crate::keys::KmsKeyId;
    use crate::test_utils::UnresponsiveServer;

    /// A key ARN of another AWS account, the form imported relayer keys take
    const CROSS_ACCOUNT_KEY_ARN: &str =
        "arn:aws:kms:us-east-1:592080917095:key/1234abcd-12ab-34cd-56ef-1234567890ab";

    /// The KMS key id passes unchanged to `GetPublicKey` and `Sign`, so a key
    /// of another account works through its ARN.
    #[tokio::test]
    async fn signs_with_the_key_it_was_loaded_by() -> eyre::Result<()> {
        let signing_key = SigningKey::random(&mut rand::rngs::OsRng);
        let kms = StubKms::spawn(signing_key.clone()).await?;
        let keys = KmsKeys::from_aws_config(
            &aws_config_for(&kms.url, 1),
            &KmsKeysConfig {
                kms_timeout: Duration::from_secs(5),
            },
        );
        let key_id: KmsKeyId = CROSS_ACCOUNT_KEY_ARN.parse()?;

        let signer = keys.load_signer(key_id.as_str().to_string()).await?;
        let raw_tx = signer
            .raw_signed_tx(TxEip1559 {
                chain_id: 480,
                nonce: 7,
                gas_limit: 21_000,
                max_fee_per_gas: 2,
                max_priority_fee_per_gas: 1,
                to: TxKind::Call(Address::ZERO),
                value: U256::from(1),
                ..Default::default()
            })
            .await?;

        let expected_address =
            Address::from_public_key(signing_key.verifying_key());
        let tx = TxEnvelope::decode_2718(&mut raw_tx.as_ref())?;
        let tx = tx.as_eip1559().context("Not an EIP-1559 transaction")?;
        let recovered = tx
            .signature()
            .recover_address_from_prehash(&tx.tx().signature_hash())?;
        assert_eq!(signer.address(), expected_address);
        assert_eq!(recovered, expected_address);
        let requests = kms.requests();
        let operations: Vec<&str> = requests
            .iter()
            .map(|(operation, _)| operation.as_str())
            .collect();
        assert_eq!(
            operations,
            ["TrentService.GetPublicKey", "TrentService.Sign"]
        );
        assert!(
            requests
                .iter()
                .all(|(_, key_id)| key_id == CROSS_ACCOUNT_KEY_ARN),
            "{requests:?}"
        );

        Ok(())
    }

    /// A KMS endpoint that holds one secp256k1 key, answers `GetPublicKey`
    /// and `Sign` for any key id, and records each request's operation and
    /// key id
    struct StubKms {
        url: String,
        requests: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl StubKms {
        async fn spawn(signing_key: SigningKey) -> eyre::Result<Self> {
            let requests = Arc::new(Mutex::new(vec![]));
            let recorded = requests.clone();

            let endpoint = poem::endpoint::make(move |req: poem::Request| {
                let signing_key = signing_key.clone();
                let recorded = recorded.clone();

                async move {
                    let operation = req
                        .header("x-amz-target")
                        .unwrap_or_default()
                        .to_string();
                    let body: Value = serde_json::from_slice(
                        &req.into_body().into_vec().await.unwrap(),
                    )
                    .unwrap();
                    let key_id = body["KeyId"].as_str().unwrap().to_string();
                    recorded
                        .lock()
                        .unwrap()
                        .push((operation.clone(), key_id.clone()));

                    let response = match operation.as_str() {
                        "TrentService.GetPublicKey" => json!({
                            "KeyId": key_id,
                            "PublicKey": base64(&public_key_der(&signing_key)),
                            "KeySpec": "ECC_SECG_P256K1",
                            "KeyUsage": "SIGN_VERIFY",
                        }),
                        "TrentService.Sign" => {
                            let digest =
                                base64::engine::general_purpose::STANDARD
                                    .decode(body["Message"].as_str().unwrap())
                                    .unwrap();
                            let signature: Signature =
                                signing_key.sign_prehash(&digest).unwrap();

                            json!({
                                "KeyId": key_id,
                                "Signature": base64(signature.to_der().as_bytes()),
                                "SigningAlgorithm": "ECDSA_SHA_256",
                            })
                        }
                        _ => panic!("Unexpected KMS operation {operation}"),
                    };

                    poem::Response::builder()
                        .content_type("application/x-amz-json-1.1")
                        .body(response.to_string())
                }
            });
            let acceptor = poem::listener::TcpListener::bind("127.0.0.1:0")
                .into_acceptor()
                .await?;
            let addr = acceptor.local_addr()[0]
                .as_socket_addr()
                .copied()
                .ok_or_else(|| eyre::eyre!("Stub KMS has no socket address"))?;
            tokio::spawn(
                poem::Server::new_with_acceptor(acceptor).run(endpoint),
            );

            Ok(Self {
                url: format!("http://{addr}"),
                requests,
            })
        }

        fn requests(&self) -> Vec<(String, String)> {
            self.requests.lock().unwrap().clone()
        }
    }

    /// Encodes the public key of `signing_key` as the DER SubjectPublicKeyInfo
    /// that KMS returns.
    fn public_key_der(signing_key: &SigningKey) -> Vec<u8> {
        // The SubjectPublicKeyInfo header of an uncompressed secp256k1 key
        const SPKI_PREFIX: [u8; 23] =
            hex_literal::hex!("3056301006072a8648ce3d020106052b8104000a034200");
        let point = signing_key.verifying_key().to_encoded_point(false);

        [&SPKI_PREFIX[..], point.as_bytes()].concat()
    }

    fn base64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
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
