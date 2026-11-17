mod common;

use std::collections::HashMap;

use serde_json::{json, Value};
use tx_sitter::api_key::ApiKey;
use tx_sitter::broadcast_utils::gas_estimation::FeesEstimate;
use tx_sitter_client::apis::relayer_v1_api::{
    CreateTransactionParams, GetTransactionParams,
};

use crate::common::prelude::*;

#[tokio::test]
async fn v2_send_is_idempotent_by_tx_id() -> eyre::Result<()> {
    setup_tracing();

    let service = TestService::spawn(api_service()).await?;
    let client = service.v2_client(&service.api_key);

    let request = send_tx_request(
        Some("tx-1"),
        Some(
            json!({ "transactionType": "SafeDeployment", "user": { "id": 7 } }),
        ),
    );

    let SendTxResponse { tx_id } = send_tx_v2(&client, request.clone()).await?;
    assert_eq!(tx_id, "tx-1");
    assert_eq!(service.relayer_nonce().await?, 1);

    let repeat = send_tx_v2(&client, request.clone()).await?;
    assert_eq!(repeat.tx_id, "tx-1");
    assert_eq!(service.relayer_nonce().await?, 1, "A repeat took a nonce");

    let conflicting_requests = [
        SendTxRequestV2 {
            metadata: Some(metadata(json!({ "transactionType": "Other" }))),
            ..request.clone()
        },
        SendTxRequestV2 {
            metadata: None,
            ..request.clone()
        },
        SendTxRequestV2 {
            value: U256::from(1).into(),
            ..request.clone()
        },
    ];
    for conflicting_request in conflicting_requests {
        let error = expect_v2_error(
            send_tx_v2(&client, conflicting_request).await,
            StatusCode::CONFLICT,
        )?;
        assert_eq!(error.error_id, "transaction_already_exists");
    }
    assert_eq!(service.relayer_nonce().await?, 1);

    let tx = get_tx_v2(&client, "tx-1").await?;
    assert_eq!(tx.nonce, 0);
    assert_eq!(tx.value, request.value);
    assert_eq!(tx.metadata, request.metadata);

    let first = send_tx_v2(&client, send_tx_request(None, None)).await?;
    let second = send_tx_v2(&client, send_tx_request(None, None)).await?;
    assert_ne!(first.tx_id, second.tx_id);
    assert_eq!(service.relayer_nonce().await?, 3);

    Ok(())
}

#[tokio::test]
async fn v2_concurrent_identical_sends_store_one_tx() -> eyre::Result<()> {
    setup_tracing();

    let service = TestService::spawn(api_service()).await?;
    let client = service.v2_client(&service.api_key);
    let request = send_tx_request(Some("tx-1"), None);

    let responses = futures::future::try_join_all(
        (0..10).map(|_| send_tx_v2(&client, request.clone())),
    )
    .await?;

    assert!(responses.iter().all(|response| response.tx_id == "tx-1"));
    assert_eq!(service.relayer_nonce().await?, 1);

    Ok(())
}

#[tokio::test]
async fn v2_identical_and_different_sends_race() -> eyre::Result<()> {
    setup_tracing();

    let service = TestService::spawn(api_service()).await?;
    let client = service.v2_client(&service.api_key);

    for round in 0..5 {
        let tx_id = format!("tx-{round}");
        let request = send_tx_request(Some(&tx_id), None);
        let different = SendTxRequestV2 {
            value: U256::from(1).into(),
            ..request.clone()
        };

        let (sent, sent_different) = tokio::join!(
            send_tx_v2(&client, request),
            send_tx_v2(&client, different)
        );

        let (created, refused) = match (sent, sent_different) {
            (Ok(created), refused) | (refused, Ok(created)) => {
                (created, refused)
            }
            other => eyre::bail!("No send succeeded: {other:?}"),
        };
        assert_eq!(created.tx_id, tx_id);
        let error = expect_v2_error(refused, StatusCode::CONFLICT)?;
        assert_eq!(error.error_id, "transaction_already_exists");
        assert_eq!(service.relayer_nonce().await?, round + 1);
    }

    Ok(())
}

#[tokio::test]
async fn v2_repeat_succeeds_while_its_relayer_refuses_sends() -> eyre::Result<()>
{
    setup_tracing();

    let service = TestService::spawn(api_service()).await?;
    let client = service.v2_client(&service.api_key);
    let request = send_tx_request(Some("tx-1"), None);
    send_tx_v2(&client, request.clone()).await?;

    service
        .update_relayer(&RelayerConfig {
            enabled: false,
            ..default_relayer()
        })
        .await?;
    send_tx_v2(&client, request.clone()).await?;
    let error = expect_v2_error(
        send_tx_v2(&client, send_tx_request(Some("tx-2"), None)).await,
        StatusCode::FORBIDDEN,
    )?;
    assert_eq!(error.error_id, "relayer_disabled");

    // Room for one more transaction than the one queued
    service
        .update_relayer(&RelayerConfig {
            max_inflight_txs: 1,
            max_queued_txs: 2,
            ..default_relayer()
        })
        .await?;
    send_tx_v2(&client, send_tx_request(Some("tx-2"), None)).await?;
    send_tx_v2(&client, request).await?;
    let error = expect_v2_error(
        send_tx_v2(&client, send_tx_request(Some("tx-3"), None)).await,
        StatusCode::TOO_MANY_REQUESTS,
    )?;
    assert_eq!(error.error_id, "queue_full");

    assert_eq!(service.relayer_nonce().await?, 2);

    Ok(())
}

#[tokio::test]
async fn v2_refuses_a_relayer_without_funds() -> eyre::Result<()> {
    setup_tracing();

    // Fee estimates come from indexed blocks
    let service = TestService::spawn(ServiceBuilder::default()).await?;

    let error = expect_v2_error(
        send_tx_v2(
            &service.v2_client(&service.other_relayer.api_key),
            send_tx_request(Some("tx-1"), None),
        )
        .await,
        StatusCode::UNPROCESSABLE_ENTITY,
    )?;
    assert_eq!(error.error_id, "insufficient_funds");

    Ok(())
}

#[tokio::test]
async fn v2_send_fails_with_503_while_the_rpc_is_down() -> eyre::Result<()> {
    setup_tracing();

    let (service, api_key) = spawn_with_relayer_on_unresponsive_chain(
        api_service().rpc_timeout(Duration::from_secs(1)),
    )
    .await?;
    let client = service.v2_client(&api_key);

    let error = expect_v2_error(
        send_tx_v2(&client, send_tx_request(Some("tx-1"), None)).await,
        StatusCode::SERVICE_UNAVAILABLE,
    )?;
    assert_eq!(error.error_id, "balance_unavailable");

    expect_v2_error(get_tx_v2(&client, "tx-1").await, StatusCode::NOT_FOUND)?;

    Ok(())
}

/// The RPC URL usually embeds a provider API key, so a failed balance check
/// must not return it.
#[tokio::test]
async fn v1_balance_check_error_hides_the_rpc_url() -> eyre::Result<()> {
    setup_tracing();

    let (service, api_key) = spawn_with_relayer_on_unresponsive_chain(
        api_service().rpc_timeout(Duration::from_secs(1)),
    )
    .await?;

    let response = reqwest::Client::new()
        .post(format!("{}/1/api/{api_key}/tx", service.base_url()))
        .json(&serde_json::json!({
            "to": "0x0000000000000000000000000000000000000001",
            "value": "0",
            "gasLimit": "21000",
        }))
        .send()
        .await?;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = response.text().await?;
    assert!(!body.contains("http://"), "RPC URL in {body:?}");
    assert!(!body.contains("127.0.0.1"), "RPC address in {body:?}");

    Ok(())
}

#[tokio::test]
async fn v2_get_returns_only_the_relayers_transactions() -> eyre::Result<()> {
    setup_tracing();

    let service = TestService::spawn(api_service()).await?;
    let client = service.v2_client(&service.api_key);

    let other_client = service.v2_client(&service.other_relayer.api_key);

    let sent_metadata = json!({
        "transactionType": "WorldIdVerification",
        "rows": [{ "id": "a1", "amount": "1000000000000000000000000" }],
        "nested": { "flag": true, "count": 3, "ratio": 0.5, "none": null },
        "unicode": "zażółć 🦀"
    });
    let request = send_tx_request(Some("tx-1"), Some(sent_metadata.clone()));
    send_tx_v2(&client, request.clone()).await?;

    let tx = get_tx_v2(&client, "tx-1").await?;
    assert_eq!(tx.tx_id, "tx-1");
    assert_eq!(tx.relayer_id, DEFAULT_RELAYER_ID);
    assert_eq!(tx.chain_id, DEFAULT_ANVIL_CHAIN_ID as i32);
    assert_eq!(tx.to, request.to);
    assert_eq!(tx.data, request.data.clone().unwrap());
    assert_eq!(tx.gas_limit, request.gas_limit);
    assert_eq!(tx.priority, TransactionPriority::Fast);
    assert_eq!(tx.status, TxStatusV2::Queued);
    assert_eq!(tx.tx_hash, None);
    assert_eq!(tx.mined_at, None);
    assert_eq!(
        serde_json::to_value(&tx.metadata)?,
        sent_metadata,
        "Metadata changed"
    );
    let created_at = chrono::DateTime::parse_from_rfc3339(&tx.created_at)?;
    let age = chrono::Utc::now().signed_duration_since(created_at);
    assert!(age < chrono::Duration::minutes(1), "Created {age} ago");

    for (client, tx_id) in [(&other_client, "tx-1"), (&client, "unknown")] {
        let error = expect_v2_error(
            get_tx_v2(client, tx_id).await,
            StatusCode::NOT_FOUND,
        )?;
        assert_eq!(error.error_id, "not_found");
    }

    let error = expect_v2_error(
        send_tx_v2(&other_client, request).await,
        StatusCode::CONFLICT,
    )?;
    assert_eq!(error.error_id, "transaction_already_exists");

    Ok(())
}

#[tokio::test]
async fn v2_returns_metadata_exactly_as_sent() -> eyre::Result<()> {
    setup_tracing();

    let service = TestService::spawn(api_service()).await?;
    let http = reqwest::Client::new();
    let send = |tx_id: &str, metadata: &str| {
        http.post(format!("{}/2/tx", service.base_url()))
            .bearer_auth(&service.api_key)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"to":"{ARBITRARY_ADDRESS}","value":"0","gasLimit":"21000","txId":"{tx_id}","metadata":{metadata}}}"#
            ))
            .send()
    };
    let metadata =
        r#"{ "b": 18446744073709551617, "a": [1.10, 1e2, -0.0], "s": "é" }"#;
    let adjacent =
        r#"{ "b": 18446744073709551616, "a": [1.10, 1e2, -0.0], "s": "é" }"#;

    let response = send("tx-1", metadata).await?;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        response.text().await?
    );
    let response = send("tx-1", metadata).await?;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        response.text().await?
    );
    let response = send("tx-1", adjacent).await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let tx: HashMap<String, Box<serde_json::value::RawValue>> = http
        .get(format!("{}/2/tx/tx-1", service.base_url()))
        .bearer_auth(&service.api_key)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(tx["metadata"].get(), metadata);

    Ok(())
}

#[tokio::test]
async fn v1_and_v2_share_tx_ids() -> eyre::Result<()> {
    setup_tracing();

    let service = TestService::spawn(api_service()).await?;
    let client = service.v2_client(&service.api_key);

    let v1_request = SendTxRequest {
        to: ARBITRARY_ADDRESS.into(),
        value: U256::ZERO.into(),
        gas_limit: U256::from(21_000).into(),
        tx_id: Some("v1-tx".to_string()),
        ..Default::default()
    };
    send_v1_tx(&service, v1_request.clone()).await?;
    send_tx_v2(&client, send_tx_request(Some("v2-tx"), None)).await?;

    let tx = get_tx_v2(&client, "v1-tx").await?;
    assert_eq!(tx.metadata, None);
    assert_eq!(tx.status, TxStatusV2::Queued);

    let v1_tx = tx_sitter_client::apis::relayer_v1_api::get_transaction(
        &service.client,
        GetTransactionParams {
            api_token: service.api_key.clone(),
            tx_id: "v2-tx".to_string(),
        },
    )
    .await?;
    assert_eq!(v1_tx.nonce, 1);

    expect_v2_error(
        send_tx_v2(&client, send_tx_request(Some("v1-tx"), None)).await,
        StatusCode::CONFLICT,
    )?;
    let response = send_v1_tx(
        &service,
        SendTxRequest {
            tx_id: Some("v2-tx".to_string()),
            ..v1_request
        },
    )
    .await;
    match response {
        Err(tx_sitter_client::apis::Error::ResponseError(response)) => {
            assert_eq!(response.status, StatusCode::CONFLICT);
            assert_eq!(
                response.content,
                r#"{"errorId":"transaction_already_exists","errorMessage":"Transaction with same id already exists."}"#
            );
        }
        other => eyre::bail!("Expected 409, got {other:?}"),
    }
    assert_eq!(service.relayer_nonce().await?, 2);

    Ok(())
}

#[tokio::test]
async fn v2_requires_a_bearer_api_key() -> eyre::Result<()> {
    setup_tracing();

    let service = TestService::spawn(api_service()).await?;
    let http = reqwest::Client::new();
    let base_url = service.base_url();
    let body = json!({
        "to": ARBITRARY_ADDRESS,
        "value": "0",
        "gasLimit": "21000",
        "txId": "tx-1"
    });

    let unknown_api_key = ApiKey::random(DEFAULT_RELAYER_ID).reveal()?;
    let unauthorized_requests = [
        http.post(format!("{base_url}/2/tx")),
        http.post(format!("{base_url}/2/tx"))
            .bearer_auth("not-an-api-key"),
        http.post(format!("{base_url}/2/tx"))
            .bearer_auth(&unknown_api_key),
        http.post(format!("{base_url}/2/tx"))
            .basic_auth(&service.api_key, None::<&str>),
        http.get(format!("{base_url}/2/tx/tx-1")),
        http.get(format!("{base_url}/2/tx/tx-1"))
            .bearer_auth(&unknown_api_key),
    ];
    for request in unauthorized_requests {
        let response = request.json(&body).send().await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let error: ErrorResponseBody = response.json().await?;
        assert_eq!(error.error_id, "unauthorized");
    }

    let api_key_in_path = [
        http.post(format!("{base_url}/2/{}/tx", service.api_key)),
        http.get(format!("{base_url}/2/{}/tx/tx-1", service.api_key)),
    ];
    for request in api_key_in_path {
        let response = request.json(&body).send().await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    assert_eq!(service.relayer_nonce().await?, 0);

    let response = http
        .post(format!("{base_url}/2/tx"))
        .bearer_auth(&service.api_key)
        .json(&body)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);

    Ok(())
}

#[tokio::test]
async fn v2_rejects_invalid_and_oversized_requests() -> eyre::Result<()> {
    setup_tracing();

    let service = TestService::spawn(api_service()).await?;
    let http = reqwest::Client::new();
    let url = format!("{}/2/tx", service.base_url());
    let request = |fields: Value| {
        let mut body = json!({
            "to": ARBITRARY_ADDRESS,
            "value": "0",
            "gasLimit": "21000"
        });
        body.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());

        http.post(&url).bearer_auth(&service.api_key).json(&body)
    };

    let rejected_requests = [
        (
            request(json!({ "metadata": { "k": "x".repeat(64 * 1024) } })),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
        (
            request(json!({ "metadata": "not an object" })),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
        (
            request(json!({ "blobs": [[0, 255]] })),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
        (
            request(json!({ "txId": "x".repeat(256) })),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
        (
            request(json!({ "gasLimit": "18446744073709551616" })),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
        (
            request(
                json!({ "data": format!("0x{}", "00".repeat(300 * 1024)) }),
            ),
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
        ),
        (
            http.post(&url)
                .bearer_auth(&service.api_key)
                .header("content-type", "text/plain")
                .body("{}"),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
        ),
    ];
    for (request, expected_status, expected_error_id) in rejected_requests {
        let response = request.send().await?;
        let status = response.status();
        let body = response.text().await?;
        assert_eq!(status, expected_status, "{body}");
        let error: ErrorResponseBody = serde_json::from_str(&body)?;
        assert_eq!(error.error_id, expected_error_id, "{body}");
    }

    assert_eq!(service.relayer_nonce().await?, 0);

    Ok(())
}

#[tokio::test]
async fn v2_get_reports_a_mined_transaction() -> eyre::Result<()> {
    setup_tracing();

    let service = TestService::spawn(ServiceBuilder::default()).await?;
    let client = service.v2_client(&service.api_key);
    let provider = setup_provider(service.anvil.endpoint()).await?;

    let value: U256 = parse_units("1", "ether")?.into();
    let request = SendTxRequestV2 {
        value: value.into(),
        data: None,
        ..send_tx_request(
            Some("tx-1"),
            Some(json!({ "transactionType": "Transfer" })),
        )
    };
    send_tx_v2(&client, request.clone()).await?;

    let tx = await_mined(&client, "tx-1").await?;
    assert_eq!(tx.metadata, request.metadata);

    let tx_hash = tx.tx_hash.expect("A mined transaction has a hash");
    let receipt = provider.get_transaction_receipt(tx_hash.0).await?;
    assert!(receipt.is_some(), "No receipt for {tx_hash:?}");

    await_balance(&provider, value, ARBITRARY_ADDRESS).await?;

    Ok(())
}

/// A service that serves the API and runs no background tasks, so that
/// sent transactions stay queued
fn api_service() -> ServiceBuilder {
    ServiceBuilder::default().role(ServiceRole::Api)
}

fn send_tx_request(
    tx_id: Option<&str>,
    tx_metadata: Option<Value>,
) -> SendTxRequestV2 {
    SendTxRequestV2 {
        data: Some(vec![0xde, 0xad].into()),
        priority: Some(TransactionPriority::Fast),
        tx_id: tx_id.map(str::to_string),
        metadata: tx_metadata.map(metadata),
        ..SendTxRequestV2::new(
            ARBITRARY_ADDRESS.into(),
            U256::ZERO.into(),
            U256::from(21_000).into(),
        )
    }
}

fn metadata(object: Value) -> HashMap<String, Value> {
    serde_json::from_value(object).expect("Metadata must be an object")
}

async fn send_v1_tx(
    service: &TestService,
    request: SendTxRequest,
) -> Result<
    SendTxResponse,
    tx_sitter_client::apis::Error<
        tx_sitter_client::apis::relayer_v1_api::CreateTransactionError,
    >,
> {
    tx_sitter_client::apis::relayer_v1_api::create_transaction(
        &service.client,
        CreateTransactionParams {
            api_token: service.api_key.clone(),
            send_tx_request: request,
        },
    )
    .await
}

/// Spawns a service from `builder` with a relayer on a chain whose RPC never
/// answers, and fee estimates for that chain so that sends check the
/// relayer's balance. Returns the service and an API key of the relayer.
async fn spawn_with_relayer_on_unresponsive_chain(
    builder: ServiceBuilder,
) -> eyre::Result<(TestService, String)> {
    const CHAIN_ID: u64 = 2;

    let rpc = UnresponsiveRpc::spawn().await?;
    let relayer = TestRelayer::random(CHAIN_ID);
    let service = TestService::spawn(
        builder
            .network(rpc.network(CHAIN_ID))
            .relayer(relayer.config),
    )
    .await?;

    let gwei: U256 = parse_units("1", "gwei")?.into();
    let db = &service.db;
    db.save_block(1, CHAIN_ID, chrono::Utc::now(), &[]).await?;
    db.save_block_fees(
        1,
        CHAIN_ID,
        &FeesEstimate {
            base_fee_per_gas: gwei,
            percentile_fees: vec![gwei; 5],
        },
        gwei,
        None,
    )
    .await?;

    Ok((service, relayer.api_key))
}

async fn await_mined(
    client: &Configuration,
    tx_id: &str,
) -> eyre::Result<GetTxResponseV2> {
    for _ in 0..30 {
        let tx = get_tx_v2(client, tx_id).await?;

        if matches!(tx.status, TxStatusV2::Mined | TxStatusV2::Finalized) {
            assert!(tx.mined_at.is_some(), "A mined tx has no minedAt");

            return Ok(tx);
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    eyre::bail!("Transaction {tx_id} was not mined in time");
}
