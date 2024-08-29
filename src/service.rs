use std::net::SocketAddr;
use std::sync::Arc;

use ethers::signers::{Signer, Wallet};
use tokio::task::JoinHandle;

use crate::app::App;
use crate::config::Config;
use crate::keys::local_keys::signing_key_from_hex;
use crate::task_runner::TaskRunner;
use crate::tasks;

pub struct Service {
    _app: Arc<App>,
    local_addr: SocketAddr,
    server_handle: JoinHandle<eyre::Result<()>>,
}

impl Service {
    pub async fn new(config: Config) -> eyre::Result<Self> {
        let app = Arc::new(App::new(config).await?);

        tracing::info!("Getting network chain ids");
        let chain_ids = app.db.get_network_chain_ids().await?;

        tracing::info!("Spawning tasks");
        let task_runner = TaskRunner::new(app.clone());
        task_runner.add_task("Broadcast transactions", tasks::broadcast_txs);
        task_runner.add_task("Escalate transactions", tasks::escalate_txs_task);
        task_runner.add_task("Prune blocks", tasks::prune_blocks);
        task_runner.add_task("Prune transactions", tasks::prune_txs);
        task_runner.add_task("Finalize transactions", tasks::finalize_txs);
        task_runner.add_task("Handle soft reorgs", tasks::handle_soft_reorgs);
        task_runner.add_task("Handle hard reorgs", tasks::handle_hard_reorgs);

        if let Some(telemetry_config) = app.config.service.telemetry.as_ref() {
            if telemetry_config.metrics.is_some() {
                task_runner.add_task("Emit metrics", tasks::emit_metrics);
            }
        }

        for chain_id in chain_ids {
            Self::spawn_chain_tasks(&task_runner, chain_id)?;
        }

        let server = crate::server::spawn_server(app.clone()).await?;
        let local_addr = server.local_addr();
        let server_handle = server.server_handle;

        initialize_predefined_values(&app).await?;

        Ok(Self {
            _app: app,
            local_addr,
            server_handle,
        })
    }

    pub fn spawn_chain_tasks(
        task_runner: &TaskRunner<App>,
        chain_id: u64,
    ) -> eyre::Result<()> {
        task_runner.add_task(
            format!("Index blocks (chain id: {})", chain_id),
            move |app| crate::tasks::index::index_chain(app, chain_id),
        );

        task_runner.add_task(
            format!("Estimate fees (chain id: {})", chain_id),
            move |app| crate::tasks::index::estimate_gas(app, chain_id),
        );

        Ok(())
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn wait(self) -> eyre::Result<()> {
        self.server_handle.await??;

        Ok(())
    }

    pub async fn is_estimates_ready_for_chain(&self, chain_id: u64) -> bool {
        let res = self
            ._app
            .db
            .get_latest_block_fees_by_chain_id(chain_id)
            .await;
        match res {
            Ok(res) => res.is_some(),
            Err(_) => false,
        }
    }
}

async fn initialize_predefined_values(
    app: &Arc<App>,
) -> Result<(), eyre::Error> {
    if app.config.service.predefined.is_some() && !app.config.keys.is_local() {
        eyre::bail!("Predefined relayers are only supported with local keys");
    }

    let Some(predefined) = app.config.service.predefined.as_ref() else {
        return Ok(());
    };

    tracing::warn!("Running with predefined values is not recommended in a production environment");

    if app
        .db
        .get_network(predefined.network.chain_id)
        .await?
        .is_none()
    {
        app.db
            .upsert_network(
                predefined.network.chain_id,
                &predefined.network.name,
                &predefined.network.http_rpc,
                &predefined.network.ws_rpc,
            )
            .await?;

        let task_runner = TaskRunner::new(app.clone());
        Service::spawn_chain_tasks(&task_runner, predefined.network.chain_id)?;
    }

    let secret_key = signing_key_from_hex(&predefined.relayer.key_id)?;

    let signer = Wallet::from(secret_key);
    let address = signer.address();

    if app.db.get_relayer(&predefined.relayer.id).await?.is_none() {
        app.db
            .create_relayer(
                &predefined.relayer.id,
                &predefined.relayer.name,
                predefined.relayer.chain_id,
                &predefined.relayer.key_id,
                address,
            )
            .await?;
    }

    app.db
        .upsert_api_key(
            predefined.relayer.api_key.relayer_id(),
            predefined.relayer.api_key.api_key_secret_hash(),
        )
        .await?;

    Ok(())
}
