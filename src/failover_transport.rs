use std::ops::ControlFlow;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
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
pub const DEMOTION_PERIOD: Duration = Duration::from_secs(30);

/// An alloy transport that sends each request to the HTTP RPCs of a network
/// in order of preference, and moves on to the next RPC when one fails.
///
/// An RPC fails a request when it can't be reached, times out, or answers
/// with HTTP 5xx or 429. Any other answer, such as another HTTP error status,
/// a JSON-RPC error or a body that isn't JSON-RPC, is returned without trying
/// another RPC, because another RPC would most likely answer the same. A
/// request tries every RPC at most once, so with `n` RPCs it takes at most
/// `n` times the timeout of the HTTP client.
///
/// An RPC that fails a request is demoted for a period, [`DEMOTION_PERIOD`]
/// by default: requests try it after the others. After the period a single
/// request tries it in its place again, as a probe, while the other requests
/// keep trying it last for another period, so that an RPC that hangs delays
/// one request per period rather than every request. A probe that gets an
/// answer restores the RPC. When every RPC fails, the request fails with the
/// error of the last one.
///
/// Errors carry no RPC URLs, because those usually embed provider API keys,
/// and [`describe_error`] describes them for logs.
#[derive(Clone)]
pub struct FailoverTransport {
    chain_id: u64,
    rpcs: Arc<[Rpc]>,
    demotion_period: Duration,
}

struct Rpc {
    transport: Http<reqwest::Client>,
    /// Until when requests try the RPC last, `None` while it isn't demoted
    demoted_until: Mutex<Option<Instant>>,
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
                demoted_until: Mutex::new(None),
            })
            .collect();

        Self {
            chain_id,
            rpcs,
            demotion_period: DEMOTION_PERIOD,
        }
    }

    async fn send(
        self,
        request: RequestPacket,
    ) -> Result<ResponsePacket, TransportError> {
        let mut demoted = vec![];
        let mut last_error = None;

        for (rpc_index, rpc) in self.rpcs.iter().enumerate() {
            if !rpc.claim(Instant::now(), self.demotion_period) {
                demoted.push((rpc_index, rpc));
                continue;
            }

            match self.try_rpc(rpc_index, rpc, &request).await {
                ControlFlow::Break(result) => return result,
                ControlFlow::Continue(err) => last_error = Some(err),
            }
        }

        for (rpc_index, rpc) in demoted {
            match self.try_rpc(rpc_index, rpc, &request).await {
                ControlFlow::Break(result) => return result,
                ControlFlow::Continue(err) => last_error = Some(err),
            }
        }

        Err(last_error.unwrap_or_else(|| {
            TransportErrorKind::custom_str("The network has no HTTP RPCs")
        }))
    }

    /// Sends `request` to `rpc`, and breaks with the result once the RPC
    /// answers, or continues with the error when it fails the request, which
    /// demotes it.
    async fn try_rpc(
        &self,
        rpc_index: usize,
        rpc: &Rpc,
        request: &RequestPacket,
    ) -> ControlFlow<Result<ResponsePacket, TransportError>, TransportError>
    {
        let err = match rpc.transport.clone().call(request.clone()).await {
            Ok(response) => {
                rpc.set_demoted_until(None);

                return ControlFlow::Break(Ok(response));
            }
            Err(err) => without_url(err),
        };

        if !is_rpc_failure(&err) {
            rpc.set_demoted_until(None);

            return ControlFlow::Break(Err(err));
        }

        metrics::counter!(
            "rpc_failures",
            "chain_id" => self.chain_id.to_string(),
            "rpc_index" => rpc_index.to_string(),
        )
        .increment(1);
        let demoted_until = Instant::now() + self.demotion_period;
        if rpc.set_demoted_until(Some(demoted_until)).is_none() {
            tracing::warn!(
                chain_id = self.chain_id,
                rpc_index,
                error = %describe_error(&err),
                "HTTP RPC failed a request, trying it last for {:?}",
                self.demotion_period
            );
        }

        ControlFlow::Continue(err)
    }

    #[cfg(test)]
    fn with_demotion_period(mut self, demotion_period: Duration) -> Self {
        self.demotion_period = demotion_period;
        self
    }
}

impl Rpc {
    /// Returns whether a request may try the RPC in its place: while the RPC
    /// isn't demoted, and once its demotion is over, when this request
    /// becomes the probe and demotes it for another `demotion_period`.
    fn claim(&self, now: Instant, demotion_period: Duration) -> bool {
        let mut demoted_until = self.lock_demoted_until();

        match *demoted_until {
            None => true,
            Some(until) if now >= until => {
                *demoted_until = Some(now + demotion_period);
                true
            }
            Some(_) => false,
        }
    }

    /// Sets until when the RPC is demoted and returns the previous value.
    fn set_demoted_until(&self, until: Option<Instant>) -> Option<Instant> {
        std::mem::replace(&mut *self.lock_demoted_until(), until)
    }

    fn lock_demoted_until(&self) -> MutexGuard<'_, Option<Instant>> {
        self.demoted_until
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

/// Returns whether `err` means the RPC is unavailable: it couldn't be
/// reached, timed out, or answered with HTTP 5xx or 429.
fn is_rpc_failure(err: &TransportError) -> bool {
    match err {
        RpcError::Transport(TransportErrorKind::HttpError(err)) => {
            err.status == 429 || err.status >= 500
        }
        RpcError::Transport(TransportErrorKind::Custom(err)) => {
            err.downcast_ref::<reqwest::Error>().is_some_and(|err| {
                err.is_connect()
                    || err.is_timeout()
                    || err.is_request()
                    || err.is_body()
            })
        }
        _ => false,
    }
}

/// Describes `err` for logs without the request's URL, which usually embeds
/// a provider API key, and with the HTTP status of an HTTP error instead of
/// its response body, which a provider may use to echo the URL.
pub fn describe_error(err: &TransportError) -> String {
    match err {
        RpcError::Transport(TransportErrorKind::HttpError(err)) => {
            format!("HTTP error {}", err.status)
        }
        RpcError::Transport(TransportErrorKind::Custom(inner)) => {
            match inner.downcast_ref::<reqwest::Error>() {
                Some(err) if err.is_timeout() => {
                    "Request timed out".to_string()
                }
                Some(err) if err.is_connect() => {
                    "Connection failed".to_string()
                }
                Some(err) => match err.status() {
                    Some(status) => format!("HTTP error {status}"),
                    None => "Request failed".to_string(),
                },
                None => err.to_string(),
            }
        }
        RpcError::DeserError { err, .. } => {
            format!("Invalid JSON-RPC response: {err}")
        }
        RpcError::ErrorResp(payload) => {
            format!("JSON-RPC error {}", payload.code)
        }
        err => err.to_string(),
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
    use test_case::test_case;

    use super::*;
    use crate::test_utils::{StubRpc, UnresponsiveServer, RPC_TIMEOUT};

    fn provider(urls: &[&str]) -> RootProvider {
        provider_with_demotion_period(urls, DEMOTION_PERIOD)
    }

    fn provider_with_demotion_period(
        urls: &[&str],
        demotion_period: Duration,
    ) -> RootProvider {
        let urls: Vec<SecretUrl> = urls
            .iter()
            .map(|url| SecretUrl::new(url.parse().unwrap()))
            .collect();
        let client = reqwest::Client::builder()
            .timeout(RPC_TIMEOUT)
            .build()
            .unwrap();
        let transport = FailoverTransport::new(1, &urls, &client)
            .with_demotion_period(demotion_period);

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
    async fn probes_a_demoted_rpc_with_a_single_request() -> eyre::Result<()> {
        let hanging = UnresponsiveServer::spawn().await?;
        let fallback = answering_rpc().await?;
        let demotion_period = RPC_TIMEOUT * 2;
        let provider = provider_with_demotion_period(
            &[&format!("http://{}", hanging.addr), &fallback.url],
            demotion_period,
        );
        assert_eq!(provider.get_block_number().await?, 42);
        assert_eq!(hanging.accepted_connections(), 1);

        tokio::time::sleep(demotion_period).await;
        let requests = 5;
        let block_numbers = futures::future::join_all(
            (0..requests).map(|_| provider.get_block_number()),
        )
        .await;

        for block_number in block_numbers {
            assert_eq!(block_number?, 42);
        }
        assert_eq!(
            hanging.accepted_connections(),
            2,
            "Every request probed the demoted RPC"
        );
        assert_eq!(fallback.requests(), 1 + requests);

        Ok(())
    }

    #[test_case(StatusCode::SERVICE_UNAVAILABLE ; "server error")]
    #[test_case(StatusCode::TOO_MANY_REQUESTS ; "rate limit")]
    #[tokio::test]
    async fn falls_back_on_an_unavailable_rpc(
        status: StatusCode,
    ) -> eyre::Result<()> {
        let failing =
            StubRpc::spawn(status, json!({ "result": "0x1" })).await?;
        let fallback = answering_rpc().await?;
        let provider = provider(&[&failing.url, &fallback.url]);

        assert_eq!(provider.get_block_number().await?, 42);
        assert_eq!(provider.get_block_number().await?, 42);

        assert_eq!(failing.requests(), 1, "The RPC wasn't demoted");
        assert_eq!(fallback.requests(), 2);

        Ok(())
    }

    #[tokio::test]
    async fn returns_a_client_error_without_demoting_the_rpc(
    ) -> eyre::Result<()> {
        let primary =
            StubRpc::spawn(StatusCode::BAD_REQUEST, json!({ "result": "0x1" }))
                .await?;
        let fallback = answering_rpc().await?;
        let provider = provider(&[&primary.url, &fallback.url]);

        for _ in 0..2 {
            let err = provider.get_block_number().await.unwrap_err();

            let RpcError::Transport(TransportErrorKind::HttpError(http_error)) =
                &err
            else {
                eyre::bail!("Expected the primary's HTTP error, got {err:?}");
            };
            assert_eq!(http_error.status, 400);
        }

        assert_eq!(primary.requests(), 2);
        assert_eq!(fallback.requests(), 0);

        Ok(())
    }

    #[test]
    fn error_descriptions_leave_out_response_bodies() {
        let http_error = TransportErrorKind::http_error(
            503,
            "upstream https://rpc.example.com/v2/provider-key failed".into(),
        );
        let deser_error = TransportError::deser_err(
            serde_json::from_str::<u64>("x").unwrap_err(),
            "https://rpc.example.com/v2/provider-key",
        );

        assert_eq!(describe_error(&http_error), "HTTP error 503");
        assert_eq!(
            describe_error(&deser_error),
            "Invalid JSON-RPC response: expected value at line 1 column 1"
        );
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
