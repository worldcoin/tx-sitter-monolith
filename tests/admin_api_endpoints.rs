mod common;

use serde_json::json;
use tx_sitter_client::apis::admin_v1_api::CreateRelayerParams;

use crate::common::prelude::*;

#[tokio::test]
async fn admin_api_can_create_get_and_list_relayer_txs() -> eyre::Result<()> {
    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (_service, client) =
        ServiceBuilder::default().build(&anvil, &db_url).await?;
    let http = reqwest::Client::new();

    let tx_id = "admin-api-test-tx";
    let create_res = http
        .post(format!(
            "{}/1/api/admin/relayer/{}/tx",
            client.base_path, DEFAULT_RELAYER_ID
        ))
        .basic_auth("", Some(""))
        .json(&json!({
            "to": ARBITRARY_ADDRESS,
            "value": "0",
            "gasLimit": "21000",
            "txId": tx_id
        }))
        .send()
        .await?;
    assert_eq!(create_res.status(), reqwest::StatusCode::OK);
    assert_eq!(create_res.json::<serde_json::Value>().await?["txId"], tx_id);

    let get_res = http
        .get(format!(
            "{}/1/api/admin/relayer/{}/tx/{}",
            client.base_path, DEFAULT_RELAYER_ID, tx_id
        ))
        .basic_auth("", Some(""))
        .send()
        .await?;
    assert_eq!(get_res.status(), reqwest::StatusCode::OK);
    let tx = get_res.json::<serde_json::Value>().await?;
    assert_eq!(tx["txId"], tx_id);
    assert_eq!(tx["relayerId"], DEFAULT_RELAYER_ID);
    assert_eq!(tx["priority"], "regular");

    let list_res = http
        .get(format!(
            "{}/1/api/admin/relayer/{}/txs?unsent=true",
            client.base_path, DEFAULT_RELAYER_ID
        ))
        .basic_auth("", Some(""))
        .send()
        .await?;
    assert_eq!(list_res.status(), reqwest::StatusCode::OK);
    let txs = list_res.json::<Vec<serde_json::Value>>().await?;
    assert!(txs.iter().any(|tx| tx["txId"] == tx_id));

    Ok(())
}

#[tokio::test]
async fn admin_api_can_delete_relayer() -> eyre::Result<()> {
    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (_service, client) =
        ServiceBuilder::default().build(&anvil, &db_url).await?;

    let CreateRelayerResponse { relayer_id, .. } =
        tx_sitter_client::apis::admin_v1_api::create_relayer(
            &client,
            CreateRelayerParams {
                create_relayer_request: CreateRelayerRequest::new(
                    "Relayer to delete".to_string(),
                    DEFAULT_ANVIL_CHAIN_ID as i32,
                ),
            },
        )
        .await?;

    let http = reqwest::Client::new();
    let delete_res = http
        .delete(format!(
            "{}/1/api/admin/relayer/{}",
            client.base_path, relayer_id
        ))
        .basic_auth("", Some(""))
        .json(&json!({ "drainTo": ARBITRARY_ADDRESS }))
        .send()
        .await?;
    assert_eq!(delete_res.status(), reqwest::StatusCode::OK);
    assert!(delete_res
        .json::<serde_json::Value>()
        .await?
        .get("drainTxHash")
        .is_none());

    let get_res = http
        .get(format!(
            "{}/1/admin/relayer/{}",
            client.base_path, relayer_id
        ))
        .basic_auth("", Some(""))
        .send()
        .await?;
    assert_eq!(get_res.status(), reqwest::StatusCode::NOT_FOUND);

    Ok(())
}
