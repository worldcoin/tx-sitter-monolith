use std::time::Duration;

use alloy::primitives::keccak256;
use alloy::providers::{Provider, RootProvider};
use alloy::transports::{RpcError, TransportError, TransportErrorKind};

use crate::db::data::RelayerInfo;
use crate::task_runner::{with_jitter, TaskContext};

const MAX_SEND_RETRIES: u32 = 3;
const INITIAL_SEND_RETRY_BACKOFF: Duration = Duration::from_millis(500);

// Lowercase fragments of the messages that geth, reth, anvil and nethermind
// reject transactions with
const ALREADY_KNOWN_MESSAGES: [&str; 3] =
    ["already known", "alreadyknown", "already imported"];
const NONCE_TOO_LOW_MESSAGES: [&str; 3] =
    ["nonce too low", "noncetoolow", "oldnonce"];
const UNDERPRICED_MESSAGES: [&str; 4] = [
    "underpriced",
    "fee too low",
    "feetoolow",
    "less than block base fee",
];

/// What came of sending a signed transaction, see [`send_raw_tx`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendOutcome {
    /// The node accepted the transaction.
    Accepted,
    /// The node already had the transaction, from an earlier send.
    AlreadyKnown,
    Failed(SendFailure),
}

/// Why the node didn't take a signed transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendFailure {
    /// The nonce is used by a mined transaction, ours or one sent with the
    /// same key elsewhere.
    NonceTooLow,
    /// The fees are below the base fee, or too close to those of the pending
    /// transaction with the same nonce.
    Underpriced,
    Timeout,
    Connection,
    RateLimited,
    /// An HTTP 5xx, or an answer that isn't JSON-RPC
    ServerError,
    /// Any other rejection, such as insufficient funds
    Rejected,
}

impl SendFailure {
    /// Returns `true` for failures that say nothing about the transaction,
    /// only that the node didn't answer.
    pub fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::Timeout
                | Self::Connection
                | Self::RateLimited
                | Self::ServerError
        )
    }

    fn reason(self) -> &'static str {
        match self {
            Self::NonceTooLow => "nonce_too_low",
            Self::Underpriced => "underpriced",
            Self::Timeout => "timeout",
            Self::Connection => "connection",
            Self::RateLimited => "rate_limited",
            Self::ServerError => "server_error",
            Self::Rejected => "rejected",
        }
    }
}

/// Sends `raw_signed_tx` of `relayer`'s transaction `tx_id` to the node
/// behind `provider`.
///
/// A [retryable](SendFailure::is_retryable) failure is retried up to three
/// times with an exponential, jittered backoff, unless shutdown is requested.
/// Sending the same bytes again is safe: a node that already has them
/// answers [`SendOutcome::AlreadyKnown`].
///
/// Every failed attempt is logged with its reason and counted in the
/// `tx_send_failures` metric, so callers only decide what to do next.
pub async fn send_raw_tx(
    provider: &RootProvider,
    ctx: &TaskContext,
    relayer: &RelayerInfo,
    tx_id: &str,
    raw_signed_tx: &[u8],
) -> SendOutcome {
    let tx_hash = keccak256(raw_signed_tx);
    let mut backoff = INITIAL_SEND_RETRY_BACKOFF;
    let mut attempt = 0;

    loop {
        attempt += 1;

        let error = match provider.send_raw_transaction(raw_signed_tx).await {
            Ok(pending_tx) => {
                if *pending_tx.tx_hash() != tx_hash {
                    tracing::error!(
                        relayer_id = relayer.id,
                        chain_id = relayer.chain_id,
                        tx_id,
                        %tx_hash,
                        node_tx_hash = %pending_tx.tx_hash(),
                        "Node accepted the transaction under another hash"
                    );
                }

                return SendOutcome::Accepted;
            }
            Err(error) => error,
        };

        let failure = match classify_send_error(&error) {
            SendOutcome::Failed(failure) => failure,
            outcome => return outcome,
        };

        metrics::counter!(
            "tx_send_failures",
            &[
                ("chain_id", relayer.chain_id.to_string()),
                ("reason", failure.reason().to_string()),
            ]
        )
        .increment(1);

        let will_retry = failure.is_retryable()
            && attempt <= MAX_SEND_RETRIES
            && !ctx.is_shutting_down();

        tracing::warn!(
            relayer_id = relayer.id,
            chain_id = relayer.chain_id,
            tx_id,
            %tx_hash,
            attempt,
            reason = failure.reason(),
            will_retry,
            error = ?error,
            "Failed to send transaction"
        );

        if will_retry {
            ctx.idle(with_jitter(backoff)).await;
            backoff *= 2;
            continue;
        }

        // A used nonce or a low fee is for indexing and escalation to settle
        if !matches!(
            failure,
            SendFailure::NonceTooLow | SendFailure::Underpriced
        ) {
            tracing::error!(
                relayer_id = relayer.id,
                chain_id = relayer.chain_id,
                tx_id,
                %tx_hash,
                attempts = attempt,
                reason = failure.reason(),
                "Gave up sending transaction"
            );
        }

        return SendOutcome::Failed(failure);
    }
}

/// Maps the error of an `eth_sendRawTransaction` call to what it means for
/// the transaction, which is never [`SendOutcome::Accepted`].
fn classify_send_error(error: &TransportError) -> SendOutcome {
    let failure = match error {
        RpcError::ErrorResp(payload) => {
            if let Some(outcome) = classify_node_message(&payload.message) {
                return outcome;
            }

            if payload.is_retry_err() {
                SendFailure::RateLimited
            } else {
                SendFailure::Rejected
            }
        }
        RpcError::Transport(TransportErrorKind::HttpError(http_error)) => {
            if let Some(outcome) = classify_node_message(&http_error.body) {
                return outcome;
            }

            match http_error.status {
                429 => SendFailure::RateLimited,
                500..=599 => SendFailure::ServerError,
                _ => SendFailure::Rejected,
            }
        }
        RpcError::Transport(TransportErrorKind::Custom(error)) => {
            match error.downcast_ref::<reqwest::Error>() {
                Some(error) if error.is_timeout() => SendFailure::Timeout,
                _ => SendFailure::Connection,
            }
        }
        RpcError::Transport(_) => SendFailure::Connection,
        RpcError::NullResp | RpcError::DeserError { .. } => {
            SendFailure::ServerError
        }
        _ => SendFailure::Rejected,
    };

    SendOutcome::Failed(failure)
}

fn classify_node_message(message: &str) -> Option<SendOutcome> {
    let message = message.to_lowercase();
    let contains_any =
        |fragments: &[&str]| fragments.iter().any(|f| message.contains(f));

    if contains_any(&ALREADY_KNOWN_MESSAGES) {
        Some(SendOutcome::AlreadyKnown)
    } else if contains_any(&NONCE_TOO_LOW_MESSAGES) {
        Some(SendOutcome::Failed(SendFailure::NonceTooLow))
    } else if contains_any(&UNDERPRICED_MESSAGES) {
        Some(SendOutcome::Failed(SendFailure::Underpriced))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use alloy::consensus::TxEip1559;
    use alloy::node_bindings::Anvil;
    use alloy::primitives::{Address, Bytes, TxKind, B256, U256};
    use alloy::rpc::client::RpcClient;
    use alloy::signers::local::PrivateKeySigner;
    use poem::listener::{Acceptor, Listener};
    use poem::{IntoResponse, Request, Response};
    use serde_json::json;
    use test_case::test_case;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::db::wrappers::address::AddressWrapper;
    use crate::keys::UniversalSigner;
    use crate::test_utils::{UnresponsiveServer, RPC_TIMEOUT};

    const GWEI: u128 = 1_000_000_000;

    #[test_case(node_error(-32000, "already known") => SendOutcome::AlreadyKnown ; "geth already known")]
    #[test_case(node_error(-32003, "transaction already imported") => SendOutcome::AlreadyKnown ; "anvil already imported")]
    #[test_case(node_error(-32010, "AlreadyKnown") => SendOutcome::AlreadyKnown ; "nethermind already known")]
    #[test_case(node_error(-32000, "nonce too low: address 0x1, tx: 0 state: 1") => SendOutcome::Failed(SendFailure::NonceTooLow) ; "geth nonce too low")]
    #[test_case(node_error(-32010, "OldNonce") => SendOutcome::Failed(SendFailure::NonceTooLow) ; "nethermind old nonce")]
    #[test_case(node_error(-32000, "replacement transaction underpriced") => SendOutcome::Failed(SendFailure::Underpriced) ; "replacement underpriced")]
    #[test_case(node_error(-32000, "transaction underpriced") => SendOutcome::Failed(SendFailure::Underpriced) ; "underpriced")]
    #[test_case(node_error(-32000, "max fee per gas less than block base fee: address 0x1, maxFeePerGas: 1, baseFee: 7") => SendOutcome::Failed(SendFailure::Underpriced) ; "below base fee")]
    #[test_case(node_error(-32010, "FeeTooLow") => SendOutcome::Failed(SendFailure::Underpriced) ; "nethermind fee too low")]
    #[test_case(node_error(-32000, "insufficient funds for gas * price + value") => SendOutcome::Failed(SendFailure::Rejected) ; "insufficient funds")]
    #[test_case(node_error(-32005, "limit exceeded") => SendOutcome::Failed(SendFailure::RateLimited) ; "rate limited by error code")]
    #[test_case(TransportErrorKind::http_error(503, String::new()) => SendOutcome::Failed(SendFailure::ServerError) ; "http 503")]
    #[test_case(TransportErrorKind::http_error(502, "<html>Bad Gateway</html>".into()) => SendOutcome::Failed(SendFailure::ServerError) ; "http 502")]
    #[test_case(TransportErrorKind::http_error(500, r#"{"error":{"code":-32000,"message":"already known"}}"#.into()) => SendOutcome::AlreadyKnown ; "http 500 with already known")]
    #[test_case(TransportErrorKind::http_error(429, String::new()) => SendOutcome::Failed(SendFailure::RateLimited) ; "http 429")]
    #[test_case(TransportErrorKind::http_error(400, "bad request".into()) => SendOutcome::Failed(SendFailure::Rejected) ; "http 400")]
    #[test_case(TransportErrorKind::backend_gone() => SendOutcome::Failed(SendFailure::Connection) ; "backend gone")]
    #[test_case(RpcError::NullResp => SendOutcome::Failed(SendFailure::ServerError) ; "null response")]
    #[test_case(TransportError::deser_err(serde_json::from_str::<()>("<").unwrap_err(), "<html>") => SendOutcome::Failed(SendFailure::ServerError) ; "not json rpc")]
    fn classifies_send_errors(error: TransportError) -> SendOutcome {
        classify_send_error(&error)
    }

    #[tokio::test]
    async fn classifies_transport_failures() -> eyre::Result<()> {
        let closed_port = {
            let listener =
                std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
            listener.local_addr()?
        };
        let unresponsive = UnresponsiveServer::spawn().await?;

        for (addr, expected) in [
            (closed_port, SendFailure::Connection),
            (unresponsive.addr, SendFailure::Timeout),
        ] {
            let error = provider(addr)?
                .send_raw_transaction(&[0x02])
                .await
                .err()
                .ok_or_else(|| eyre::eyre!("Sent to {addr}"))?;

            assert_eq!(
                classify_send_error(&error),
                SendOutcome::Failed(expected),
                "{error:?}"
            );
        }

        Ok(())
    }

    #[tokio::test]
    async fn classifies_anvil_answers() -> eyre::Result<()> {
        let anvil = Anvil::new().arg("--no-mining").try_spawn()?;
        let provider = RootProvider::new_http(anvil.endpoint_url());
        let signer = UniversalSigner::Local(PrivateKeySigner::from(
            anvil.keys()[0].clone(),
        ));
        let relayer = relayer(anvil.chain_id());
        let ctx = TaskContext::new(CancellationToken::new());

        let sign = |nonce, max_fee_per_gas, value| {
            signer.raw_signed_tx(TxEip1559 {
                chain_id: anvil.chain_id(),
                nonce,
                gas_limit: 21_000,
                max_fee_per_gas,
                max_priority_fee_per_gas: 1,
                to: TxKind::Call(Address::with_last_byte(1)),
                value: U256::from(value),
                access_list: Default::default(),
                input: Default::default(),
            })
        };
        let send = |raw_signed_tx: Bytes| {
            let (provider, ctx, relayer) = (&provider, &ctx, &relayer);
            async move {
                send_raw_tx(provider, ctx, relayer, "tx_id", &raw_signed_tx)
                    .await
            }
        };

        let tx = sign(0, 2 * GWEI, 1).await?;
        assert_eq!(send(tx.clone()).await, SendOutcome::Accepted);
        assert_eq!(send(tx.clone()).await, SendOutcome::AlreadyKnown);

        let same_fee_replacement = sign(0, 2 * GWEI, 2).await?;
        assert_eq!(
            send(same_fee_replacement).await,
            SendOutcome::Failed(SendFailure::Underpriced)
        );

        let below_base_fee = sign(1, 1, 1).await?;
        assert_eq!(
            send(below_base_fee).await,
            SendOutcome::Failed(SendFailure::Underpriced)
        );

        provider
            .raw_request::<_, String>("evm_mine".into(), ())
            .await?;
        assert_eq!(
            send(tx).await,
            SendOutcome::Failed(SendFailure::NonceTooLow)
        );

        Ok(())
    }

    #[test_case(vec![Answer::ServerError, Answer::ServerError, Answer::TxHash] => (SendOutcome::Accepted, 3) ; "until accepted")]
    #[test_case(vec![Answer::ServerError] => (SendOutcome::Failed(SendFailure::ServerError), 4) ; "a bounded number of times")]
    #[test_case(vec![Answer::ServerError, Answer::NodeError("already known")] => (SendOutcome::AlreadyKnown, 2) ; "until already known")]
    #[test_case(vec![Answer::NodeError("insufficient funds for gas * price + value")] => (SendOutcome::Failed(SendFailure::Rejected), 1) ; "not after a rejection")]
    #[test_case(vec![Answer::NodeError("nonce too low")] => (SendOutcome::Failed(SendFailure::NonceTooLow), 1) ; "not after nonce too low")]
    #[tokio::test]
    async fn retries_server_errors(
        answers: Vec<Answer>,
    ) -> (SendOutcome, usize) {
        let raw_signed_tx = [0x02, 0x01];
        let rpc = ScriptedRpc::spawn(answers, keccak256(raw_signed_tx))
            .await
            .unwrap();
        let ctx = TaskContext::new(CancellationToken::new());

        let outcome = send_raw_tx(
            &provider(rpc.addr).unwrap(),
            &ctx,
            &relayer(1),
            "tx_id",
            &raw_signed_tx,
        )
        .await;

        (outcome, rpc.calls())
    }

    #[tokio::test]
    async fn stops_retrying_once_shutdown_is_requested() -> eyre::Result<()> {
        let raw_signed_tx = [0x02, 0x01];
        let rpc = ScriptedRpc::spawn(
            vec![Answer::ServerError],
            keccak256(raw_signed_tx),
        )
        .await?;
        let shutdown = CancellationToken::new();
        shutdown.cancel();
        let ctx = TaskContext::new(shutdown);

        let outcome = send_raw_tx(
            &provider(rpc.addr)?,
            &ctx,
            &relayer(1),
            "tx_id",
            &raw_signed_tx,
        )
        .await;

        assert_eq!(outcome, SendOutcome::Failed(SendFailure::ServerError));
        assert_eq!(rpc.calls(), 1);

        Ok(())
    }

    fn node_error(code: i64, message: &str) -> TransportError {
        let payload = json!({ "code": code, "message": message });

        RpcError::ErrorResp(serde_json::from_value(payload).unwrap())
    }

    /// A provider whose requests time out after [`RPC_TIMEOUT`].
    fn provider(addr: SocketAddr) -> eyre::Result<RootProvider> {
        let client = reqwest::Client::builder().timeout(RPC_TIMEOUT).build()?;
        let url = format!("http://{addr}").parse()?;

        Ok(RootProvider::new(RpcClient::new_http_with_client(
            client, url,
        )))
    }

    fn relayer(chain_id: u64) -> RelayerInfo {
        RelayerInfo {
            id: "relayer_id".to_string(),
            name: "relayer".to_string(),
            chain_id,
            key_id: "key_id".to_string(),
            address: AddressWrapper(Address::with_last_byte(1)),
            nonce: 0,
            current_nonce: 0,
            max_inflight_txs: 5,
            max_queued_txs: 20,
            gas_price_limits: vec![],
            enabled: true,
        }
    }

    #[derive(Debug, Clone, Copy)]
    enum Answer {
        ServerError,
        NodeError(&'static str),
        /// The hash of the sent transaction
        TxHash,
    }

    /// A JSON-RPC server that answers the n-th request with the n-th of its
    /// answers, and every request after those with the last one.
    struct ScriptedRpc {
        addr: SocketAddr,
        calls: Arc<AtomicUsize>,
    }

    impl ScriptedRpc {
        async fn spawn(
            answers: Vec<Answer>,
            tx_hash: B256,
        ) -> eyre::Result<Self> {
            let acceptor =
                poem::listener::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                    .into_acceptor()
                    .await?;
            let addr = *acceptor.local_addr()[0]
                .as_socket_addr()
                .ok_or_else(|| eyre::eyre!("Not a socket address"))?;
            let calls = Arc::new(AtomicUsize::new(0));

            let counter = calls.clone();
            let endpoint = poem::endpoint::make(move |req: Request| {
                let call = counter.fetch_add(1, Ordering::SeqCst);
                let answer = answers[call.min(answers.len() - 1)];

                async move { answer_request(req, answer, tx_hash).await }
            });
            tokio::spawn(
                poem::Server::new_with_acceptor(acceptor).run(endpoint),
            );

            Ok(Self { addr, calls })
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    async fn answer_request(
        req: Request,
        answer: Answer,
        tx_hash: B256,
    ) -> Response {
        let request: serde_json::Value = match req.into_body().into_json().await
        {
            Ok(request) => request,
            Err(err) => return err.into_response(),
        };
        let id = &request["id"];

        match answer {
            Answer::ServerError => Response::builder()
                .status(poem::http::StatusCode::SERVICE_UNAVAILABLE)
                .finish(),
            Answer::NodeError(message) => poem::web::Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32000, "message": message },
            }))
            .into_response(),
            Answer::TxHash => poem::web::Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": tx_hash,
            }))
            .into_response(),
        }
    }
}
