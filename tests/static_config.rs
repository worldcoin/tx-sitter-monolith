mod common;

use tx_sitter::api_key::ApiKey;
use tx_sitter::service::Service;
use tx_sitter_client::apis::relayer_v1_api::CreateTransactionParams;
use tx_sitter_client::apis::Error;

use crate::common::prelude::*;

#[tokio::test]
async fn services_that_start_at_once_apply_the_config() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    let relayer = TestRelayer::random(DEFAULT_ANVIL_CHAIN_ID);
    // Creates and migrates the database, which services can't do at once
    let db = connect_db(&db_url).await?;

    let service = || {
        ServiceBuilder::default()
            .role(ServiceRole::Api)
            .relayer(relayer.config.clone())
            .build(&anvil, &db_url)
    };
    let (first, second) = tokio::join!(service(), service());
    let (first, _) = first?;
    let (second, _) = second?;

    for service in [&first, &second] {
        for api_key in [DEFAULT_API_KEY, &relayer.api_key] {
            assert_eq!(list_txs(service, api_key).await?, StatusCode::OK);
        }
    }

    let stored = db
        .get_relayer(&relayer.config.id)
        .await?
        .ok_or_else(|| eyre::eyre!("Relayer was not stored"))?;
    assert_eq!(stored.address.0, relayer.config.address);
    assert!(stored.enabled);

    Ok(())
}

#[tokio::test]
async fn relayer_removed_from_config_is_disabled_and_loses_its_api_keys(
) -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    let removed = TestRelayer::random(DEFAULT_ANVIL_CHAIN_ID);

    let (before, _) = ServiceBuilder::default()
        .role(ServiceRole::Api)
        .relayer(removed.config.clone())
        .build(&anvil, &db_url)
        .await?;
    assert_eq!(call_rpc(&before, &removed.api_key).await?, StatusCode::OK);
    let (after, client) = ServiceBuilder::default()
        .role(ServiceRole::Api)
        .build(&anvil, &db_url)
        .await?;

    let response = tx_sitter_client::apis::relayer_v1_api::create_transaction(
        &client,
        CreateTransactionParams {
            api_token: removed.api_key.clone(),
            send_tx_request: SendTxRequest {
                to: ARBITRARY_ADDRESS.into(),
                value: U256::ZERO.into(),
                gas_limit: U256::from(21_000).into(),
                ..Default::default()
            },
        },
    )
    .await;

    let Err(Error::ResponseError(response)) = response else {
        eyre::bail!("Expected an error response, got {response:?}");
    };
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    for service in [&before, &after] {
        assert_eq!(
            call_rpc(service, &removed.api_key).await?,
            StatusCode::UNAUTHORIZED
        );
    }
    let stored = connect_db(&db_url)
        .await?
        .get_relayer(&removed.config.id)
        .await?
        .ok_or_else(|| eyre::eyre!("Relayer was deleted"))?;
    assert!(!stored.enabled);

    Ok(())
}

#[tokio::test]
async fn disabled_relayer_rejects_every_request() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    let relayer = TestRelayer::random(DEFAULT_ANVIL_CHAIN_ID);

    let (service, _) = ServiceBuilder::default()
        .role(ServiceRole::Api)
        .relayer(RelayerConfig {
            enabled: false,
            ..relayer.config.clone()
        })
        .build(&anvil, &db_url)
        .await?;

    assert_eq!(
        list_txs(&service, &relayer.api_key).await?,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call_rpc(&service, &relayer.api_key).await?,
        StatusCode::FORBIDDEN
    );
    for path in ["/2/tx/some-tx", "/2/events"] {
        let response = reqwest::Client::new()
            .get(format!("http://{}{path}", service.local_addr()))
            .bearer_auth(&relayer.api_key)
            .send()
            .await?;

        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
        let error: ErrorResponseBody = response.json().await?;
        assert_eq!(error.error_id, "relayer_disabled", "{path}");
    }

    Ok(())
}

#[tokio::test]
async fn service_with_an_older_config_revision_refuses_to_start(
) -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    let relayer = TestRelayer::random(DEFAULT_ANVIL_CHAIN_ID);

    let (_newer, _) = ServiceBuilder::default()
        .role(ServiceRole::Api)
        .config_revision(2)
        .build(&anvil, &db_url)
        .await?;
    let result = ServiceBuilder::default()
        .role(ServiceRole::Api)
        .config_revision(1)
        .relayer(relayer.config.clone())
        .build(&anvil, &db_url)
        .await;

    let Err(err) = result else {
        eyre::bail!("The service started with a stale config");
    };
    assert!(
        err.to_string().contains(
            "The config has revision 1, but the database applied revision 2 last"
        ),
        "{err}"
    );
    let stored = connect_db(&db_url)
        .await?
        .get_relayer(&relayer.config.id)
        .await?;
    assert!(stored.is_none(), "The stale config was applied");

    Ok(())
}

#[tokio::test]
async fn api_key_removed_from_config_stops_working() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    let new_api_key = ApiKey::random(DEFAULT_RELAYER_ID).reveal()?;

    let (before, _) = ServiceBuilder::default()
        .role(ServiceRole::Api)
        .build(&anvil, &db_url)
        .await?;
    assert_eq!(list_txs(&before, DEFAULT_API_KEY).await?, StatusCode::OK);

    let (after, _) = ServiceBuilder::default()
        .role(ServiceRole::Api)
        .without_defaults()
        .network(anvil_network(&anvil))
        .relayer(RelayerConfig {
            api_key_hashes: vec![api_key_hash(&new_api_key)],
            ..default_relayer()
        })
        .build(&anvil, &db_url)
        .await?;

    for service in [&before, &after] {
        assert_eq!(
            list_txs(service, DEFAULT_API_KEY).await?,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(list_txs(service, &new_api_key).await?, StatusCode::OK);
    }

    Ok(())
}

#[tokio::test]
async fn startup_fails_when_a_key_derives_another_address() -> eyre::Result<()>
{
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    let relayer = TestRelayer::random(DEFAULT_ANVIL_CHAIN_ID).config;
    let derived_address = relayer.address;
    let relayer = RelayerConfig {
        address: ARBITRARY_ADDRESS,
        ..relayer
    };

    let result = ServiceBuilder::default()
        .role(ServiceRole::Api)
        .relayer(relayer.clone())
        .build(&anvil, &db_url)
        .await;

    let Err(err) = result else {
        eyre::bail!("The service started with a mismatched key");
    };
    assert!(
        err.to_string().contains(&format!(
            "relayer {}: expected address {ARBITRARY_ADDRESS}, but its key derives {derived_address}",
            relayer.id
        )),
        "{err}"
    );
    let stored = connect_db(&db_url).await?.get_relayer(&relayer.id).await?;
    assert!(stored.is_none(), "The rejected config was applied");

    Ok(())
}

/// Returns the status of an `eth_chainId` call through the RPC proxy of
/// `api_key`'s relayer.
async fn call_rpc(
    service: &Service,
    api_key: &str,
) -> eyre::Result<StatusCode> {
    let response = reqwest::Client::new()
        .post(format!(
            "http://{}/1/api/{api_key}/rpc",
            service.local_addr()
        ))
        .json(&serde_json::json!({
            "id": 1,
            "jsonrpc": "2.0",
            "method": "eth_chainId",
        }))
        .send()
        .await?;

    Ok(response.status())
}

/// Returns the status of listing the transactions of `api_key`'s relayer.
async fn list_txs(
    service: &Service,
    api_key: &str,
) -> eyre::Result<StatusCode> {
    let (status, _) =
        http_get(service.local_addr(), &format!("/1/api/{api_key}/txs"))
            .await?;

    Ok(status)
}
