use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use alloy::rpc::json_rpc::{RequestPacket, ResponsePacket};
use alloy::transports::http::Http;
use alloy::transports::{
    RpcError, TransportError, TransportErrorKind, TransportFut,
};
use tokio::time::Instant;
use tower::Service;

use crate::types::secret_url::SecretUrl;

/// How long an RPC that failed a request is tried only after the others
const FAILED_RPC_COOLDOWN: Duration = Duration::from_secs(30);

/// An alloy transport that sends each request to the HTTP RPCs of a network
/// in order of preference, and moves on to the next RPC when one fails.
///
/// An RPC fails a request with a transport error: it can't be reached, times
/// out, or answers with an HTTP error status or a body that isn't JSON-RPC. A
/// JSON-RPC error is an answer, so it's returned without trying another RPC.
/// A request tries every RPC at most once, so with `n` RPCs it takes at most
/// `n` times the timeout of the HTTP client.
///
/// An RPC that failed a request is tried after the others for
/// [`FAILED_RPC_COOLDOWN`], so that an RPC that hangs delays one request
/// rather than every request. When every RPC fails, the request fails with
/// the error of the last one.
///
/// Errors carry no RPC URLs, because those usually embed provider API keys.
#[derive(Clone)]
pub struct FailoverTransport {
    chain_id: u64,
    rpcs: Arc<[Rpc]>,
}

struct Rpc {
    transport: Http<reqwest::Client>,
    failed_at: Mutex<Option<Instant>>,
}

impl FailoverTransport {
    /// Creates a transport that sends requests to `urls`, in order of
    /// preference, through `client`.
    pub fn new(
        chain_id: u64,
        urls: &[SecretUrl],
        client: &reqwest::Client,
    ) -> Self {
        let rpcs = urls
            .iter()
            .map(|url| Rpc {
                transport: Http::with_client(
                    client.clone(),
                    url.expose().clone(),
                ),
                failed_at: Mutex::new(None),
            })
            .collect();

        Self { chain_id, rpcs }
    }

    async fn send(
        self,
        request: RequestPacket,
    ) -> Result<ResponsePacket, TransportError> {
        let now = Instant::now();
        let (preferred, cooling_down): (Vec<_>, Vec<_>) = self
            .rpcs
            .iter()
            .enumerate()
            .partition(|(_, rpc)| !rpc.is_cooling_down(now));

        let mut last_error = None;
        for (rpc_index, rpc) in preferred.into_iter().chain(cooling_down) {
            match rpc.transport.clone().call(request.clone()).await {
                Ok(response) => {
                    rpc.set_failed_at(None);

                    return Ok(response);
                }
                Err(err) => {
                    let err = without_url(err);

                    metrics::counter!(
                        "rpc_failures",
                        "chain_id" => self.chain_id.to_string(),
                        "rpc_index" => rpc_index.to_string(),
                    )
                    .increment(1);
                    if rpc.set_failed_at(Some(Instant::now())).is_none() {
                        tracing::warn!(
                            chain_id = self.chain_id,
                            rpc_index,
                            error = %err,
                            "HTTP RPC failed a request, trying it last for {FAILED_RPC_COOLDOWN:?}"
                        );
                    }

                    last_error = Some(err);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            TransportErrorKind::custom_str("The network has no HTTP RPCs")
        }))
    }
}

impl Rpc {
    fn is_cooling_down(&self, now: Instant) -> bool {
        self.lock_failed_at().is_some_and(|failed_at| {
            now.saturating_duration_since(failed_at) < FAILED_RPC_COOLDOWN
        })
    }

    /// Sets when the RPC last failed and returns the previous value.
    fn set_failed_at(&self, failed_at: Option<Instant>) -> Option<Instant> {
        std::mem::replace(&mut *self.lock_failed_at(), failed_at)
    }

    fn lock_failed_at(&self) -> std::sync::MutexGuard<'_, Option<Instant>> {
        self.failed_at
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl Service<RequestPacket> for FailoverTransport {
    type Response = ResponsePacket;
    type Error = TransportError;
    type Future = TransportFut<'static>;

    fn poll_ready(
        &mut self,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: RequestPacket) -> Self::Future {
        Box::pin(self.clone().send(request))
    }
}

/// Drops the URL from the reqwest error inside `err`, if there is one.
fn without_url(err: TransportError) -> TransportError {
    let RpcError::Transport(TransportErrorKind::Custom(err)) = err else {
        return err;
    };

    match err.downcast::<reqwest::Error>() {
        Ok(err) => TransportErrorKind::custom(err.without_url()),
        Err(err) => RpcError::Transport(TransportErrorKind::Custom(err)),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant as StdInstant;

    use alloy::providers::{Provider, RootProvider};
    use alloy::rpc::client::RpcClient;
    use poem::http::StatusCode;
    use serde_json::json;

    use super::*;
    use crate::test_utils::{StubRpc, UnresponsiveServer, RPC_TIMEOUT};

    fn provider(urls: &[&str]) -> RootProvider {
        let urls: Vec<SecretUrl> = urls
            .iter()
            .map(|url| SecretUrl::new(url.parse().unwrap()))
            .collect();
        let client = reqwest::Client::builder()
            .timeout(RPC_TIMEOUT)
            .build()
            .unwrap();
        let transport = FailoverTransport::new(1, &urls, &client);

        RootProvider::new(RpcClient::new(transport, false))
    }

    async fn answering_rpc() -> eyre::Result<StubRpc> {
        StubRpc::spawn(StatusCode::OK, json!({ "result": "0x2a" })).await
    }

    #[tokio::test]
    async fn tries_an_rpc_that_timed_out_last() -> eyre::Result<()> {
        let hanging = UnresponsiveServer::spawn().await?;
        let fallback = answering_rpc().await?;
        let provider =
            provider(&[&format!("http://{}", hanging.addr), &fallback.url]);

        let started = StdInstant::now();
        assert_eq!(provider.get_block_number().await?, 42);
        assert!(
            started.elapsed() >= RPC_TIMEOUT,
            "Took {:?}",
            started.elapsed()
        );

        let started = StdInstant::now();
        assert_eq!(provider.get_block_number().await?, 42);
        assert!(
            started.elapsed() < RPC_TIMEOUT,
            "Took {:?}",
            started.elapsed()
        );

        assert_eq!(hanging.accepted_connections(), 1);
        assert_eq!(fallback.requests(), 2);

        Ok(())
    }

    #[tokio::test]
    async fn falls_back_on_an_http_error_status() -> eyre::Result<()> {
        let failing = StubRpc::spawn(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({ "result": "0x1" }),
        )
        .await?;
        let fallback = answering_rpc().await?;
        let provider = provider(&[&failing.url, &fallback.url]);

        assert_eq!(provider.get_block_number().await?, 42);

        assert_eq!(failing.requests(), 1);
        assert_eq!(fallback.requests(), 1);

        Ok(())
    }

    #[tokio::test]
    async fn returns_a_json_rpc_error_without_falling_back() -> eyre::Result<()>
    {
        let primary = StubRpc::spawn(
            StatusCode::OK,
            json!({ "error": { "code": -32000, "message": "nonce too low" } }),
        )
        .await?;
        let fallback = answering_rpc().await?;
        let provider = provider(&[&primary.url, &fallback.url]);

        let err = provider.get_block_number().await.unwrap_err();

        let RpcError::ErrorResp(payload) = err else {
            eyre::bail!("Expected a JSON-RPC error, got {err:?}");
        };
        assert_eq!(payload.code, -32000);
        assert_eq!(fallback.requests(), 0);

        Ok(())
    }

    #[tokio::test]
    async fn fails_with_the_error_of_the_last_rpc() -> eyre::Result<()> {
        let failing = StubRpc::spawn(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({ "result": "0x1" }),
        )
        .await?;
        let provider =
            provider(&["http://127.0.0.1:1/v2/secret-key", &failing.url]);

        let err = provider.get_block_number().await.unwrap_err();

        let RpcError::Transport(TransportErrorKind::HttpError(http_error)) =
            &err
        else {
            eyre::bail!("Expected the last RPC's HTTP error, got {err:?}");
        };
        assert_eq!(http_error.status, 503);

        Ok(())
    }

    #[tokio::test]
    async fn errors_carry_no_rpc_url() -> eyre::Result<()> {
        let provider = provider(&["http://127.0.0.1:1/v2/secret-key"]);

        let err = provider.get_block_number().await.unwrap_err();

        let RpcError::Transport(TransportErrorKind::Custom(cause)) = &err
        else {
            eyre::bail!("Expected a connection error, got {err:?}");
        };
        assert!(cause.is::<reqwest::Error>(), "Unexpected error: {err:?}");
        assert!(
            !format!("{err:?} {err}").contains("secret-key"),
            "RPC URL leaked: {err:?}"
        );

        Ok(())
    }
}
