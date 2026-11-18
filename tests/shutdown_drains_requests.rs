mod common;

use std::sync::Arc;

use poem::listener::{Acceptor, Listener};
use poem::web::{Data, Json};
use poem::{EndpointExt, Route};
use serde_json::{json, Value};
use tokio::sync::Notify;

use crate::common::prelude::*;

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(20);

/// The JSON-RPC method that [`held_rpc`] answers only once the test releases
/// it
const HELD_METHOD: &str = "test_held";

/// An RPC node that answers [`HELD_METHOD`] once the test releases it
#[derive(Default)]
struct HeldRpc {
    received: Notify,
    release: Notify,
}

/// Holds [`HELD_METHOD`] requests and fails every other one at once, such as
/// those of the background tasks.
#[poem::handler]
async fn held_rpc(
    Data(rpc): Data<&Arc<HeldRpc>>,
    Json(request): Json<Value>,
) -> Json<Value> {
    if request["method"] != HELD_METHOD {
        return Json(json!({
            "jsonrpc": "2.0",
            "id": request["id"],
            "error": { "code": -32601, "message": "Method not found" },
        }));
    }

    rpc.received.notify_one();
    rpc.release.notified().await;

    Json(json!({ "jsonrpc": "2.0", "id": request["id"], "result": "0x1" }))
}

#[tokio::test]
async fn shutdown_lets_in_flight_requests_finish() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let rpc = Arc::new(HeldRpc::default());
    let rpc_acceptor = poem::listener::TcpListener::bind("127.0.0.1:0")
        .into_acceptor()
        .await?;
    let held_rpc_url = format!(
        "http://{}",
        rpc_acceptor.local_addr()[0]
            .as_socket_addr()
            .expect("RPC listener has a socket address")
    );
    tokio::spawn(
        poem::Server::new_with_acceptor(rpc_acceptor)
            .run(Route::new().at("/", poem::post(held_rpc)).data(rpc.clone())),
    );

    let shutdown = CancellationToken::new();
    let (service, _) = ServiceBuilder::default()
        .role(ServiceRole::Api)
        .shutdown_timeout(SHUTDOWN_TIMEOUT)
        .shutdown(shutdown.clone())
        .without_defaults()
        .network(NetworkConfig {
            http_rpcs: vec![rpc_url(&held_rpc_url)],
            ..anvil_network(&anvil)
        })
        .relayer(default_relayer())
        .build(&anvil, &db_url)
        .await?;
    let addr = service.local_addr();
    let api_key = DEFAULT_API_KEY;

    let in_flight = tokio::spawn(
        reqwest::Client::new()
            .post(format!("http://{addr}/1/api/{api_key}/rpc"))
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": HELD_METHOD,
                "params": [],
            }))
            .send(),
    );
    rpc.received.notified().await;

    shutdown.cancel();
    let service_stopped = tokio::spawn(service.wait());

    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        !service_stopped.is_finished(),
        "service stopped with a request in flight"
    );
    assert!(
        http_get(addr, "/health").await.is_err(),
        "service accepted a connection after shutdown was requested"
    );

    rpc.release.notify_one();
    let response = in_flight.await??;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert_eq!(body["result"], "0x1");

    tokio::time::timeout(SHUTDOWN_TIMEOUT, service_stopped).await???;

    Ok(())
}
