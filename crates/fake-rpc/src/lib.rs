use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use axum::extract::State;
use axum::routing::{post, IntoMakeService};
use axum::{Json, Router};
use ethers::utils::{Anvil, AnvilInstance};
use hyper::server::conn::AddrIncoming;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

pub struct DoubleAnvil {
    main_anvil: Mutex<AnvilInstance>,
    reference_anvil: Mutex<AnvilInstance>,
    held_back_txs: Mutex<Vec<JsonRpcReq>>,

    auto_advance: AtomicBool,
}

impl DoubleAnvil {
    pub async fn drop_txs(&self) -> eyre::Result<()> {
        let mut held_back_txs = self.held_back_txs.lock().await;
        held_back_txs.clear();
        Ok(())
    }

    pub async fn advance(&self) -> eyre::Result<()> {
        let mut held_back_txs = self.held_back_txs.lock().await;

        for req in held_back_txs.drain(..) {
            tracing::info!(?req, "eth_sendRawTransaction");

            let response = reqwest::Client::new()
                .post(&self.main_anvil.lock().await.endpoint())
                .json(&req)
                .send()
                .await
                .unwrap();

            let resp = response.json::<Value>().await.unwrap();

            tracing::info!(?resp, "eth_sendRawTransaction.response");
        }

        Ok(())
    }

    pub fn set_auto_advance(&self, auto_advance: bool) {
        self.auto_advance
            .store(auto_advance, std::sync::atomic::Ordering::SeqCst);
    }

    pub async fn ws_endpoint(&self) -> String {
        self.main_anvil.lock().await.ws_endpoint()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct JsonRpcReq {
    pub id: u64,
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub id: u64,
    pub jsonrpc: String,
    pub result: Value,
}

async fn advance(State(anvil): State<Arc<DoubleAnvil>>) {
    anvil.advance().await.unwrap();
}

async fn rpc(
    State(anvil): State<Arc<DoubleAnvil>>,
    Json(req): Json<JsonRpcReq>,
) -> Json<Value> {
    let method = req.method.as_str();
    let anvil_instance = match method {
        "eth_sendRawTransaction" => {
            anvil.held_back_txs.lock().await.push(req.clone());

            if anvil.auto_advance.load(std::sync::atomic::Ordering::SeqCst) {
                anvil.advance().await.unwrap();
            }

            anvil.main_anvil.lock().await
        }
        "eth_getTransactionReceipt" => anvil.main_anvil.lock().await,
        "eth_getTransactionByHash" => anvil.main_anvil.lock().await,
        _ => anvil.main_anvil.lock().await,
    };

    tracing::info!(?req, "{}", method);

    let response = reqwest::Client::new()
        .post(&anvil_instance.endpoint())
        .json(&req)
        .send()
        .await
        .unwrap();

    let resp = response.json::<Value>().await.unwrap();

    tracing::info!(?resp, "{}.response", method);

    Json(resp)
}

pub async fn serve(
    port: u16,
) -> (
    Arc<DoubleAnvil>,
    axum::Server<AddrIncoming, IntoMakeService<Router>>,
) {
    let main_anvil = Anvil::new().spawn();
    let reference_anvil = Anvil::new().spawn();

    tracing::info!("Main anvil instance: {}", main_anvil.endpoint());
    tracing::info!("Reference anvil instance: {}", reference_anvil.endpoint());

    let state = Arc::new(DoubleAnvil {
        main_anvil: Mutex::new(main_anvil),
        reference_anvil: Mutex::new(reference_anvil),
        held_back_txs: Mutex::new(Vec::new()),
        auto_advance: AtomicBool::new(true),
    });

    let router = Router::new()
        .route("/", post(rpc))
        .route("/advance", post(advance))
        .with_state(state.clone())
        .layer(tower_http::trace::TraceLayer::new_for_http());

    let host = Ipv4Addr::new(127, 0, 0, 1);
    let socket_addr = SocketAddr::new(host.into(), port);

    let server =
        axum::Server::bind(&socket_addr).serve(router.into_make_service());

    (state, server)
}
