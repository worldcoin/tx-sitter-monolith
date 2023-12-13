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
    server_handle: JoinHandle<Result<(), hyper::Error>>,
}

impl Service {
    pub async fn new(config: Config) -> eyre::Result<Self> {
        let app = Arc::new(App::new(config).await?);

        let chain_ids = app.db.get_network_chain_ids().await?;

        let task_runner = TaskRunner::new(app.clone());
        task_runner.add_task("Broadcast transactions", tasks::broadcast_txs);
        task_runner.add_task("Escalate transactions", tasks::escalate_txs);
        task_runner.add_task("Prune blocks", tasks::prune_blocks);
        task_runner.add_task("Prune transactions", tasks::prune_txs);
        task_runner.add_task("Finalize transactions", tasks::finalize_txs);
        task_runner.add_task("Handle soft reorgs", tasks::handle_soft_reorgs);
        task_runner.add_task("Handle hard reorgs", tasks::handle_hard_reorgs);

        if app.config.service.statsd_enabled {
            task_runner.add_task("Emit metrics", tasks::emit_metrics);
        }

        for chain_id in chain_ids {
            Self::spawn_chain_tasks(&task_runner, chain_id)?;
        }

        let server = crate::server::spawn_server(app.clone()).await?;
        let local_addr = server.local_addr();
        let server_handle = tokio::spawn(async move {
            server.await?;
            Ok(())
        });

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
}

async fn initialize_predefined_values(
    app: &Arc<App>,
) -> Result<(), eyre::Error> {
    if !app.config.service.predefined_relayers.is_empty()
        && !app.config.keys.is_local()
    {
        eyre::bail!("Predefined relayers are only supported with local keys");
    }

    for predefined_network in &app.config.service.predefined_networks {
        app.db
            .create_network(
                predefined_network.chain_id,
                &predefined_network.name,
                &predefined_network.http_rpc,
                &predefined_network.ws_rpc,
            )
            .await?;

        let task_runner = TaskRunner::new(app.clone());
        Service::spawn_chain_tasks(&task_runner, predefined_network.chain_id)?;
    }

    for predefined_relayer in &app.config.service.predefined_relayers {
        let secret_key = signing_key_from_hex(&predefined_relayer.key_id)?;

        let signer = Wallet::from(secret_key);
        let address = signer.address();

        app.db
            .create_relayer(
                &predefined_relayer.id,
                &predefined_relayer.name,
                predefined_relayer.chain_id,
                &predefined_relayer.key_id,
                address,
            )
            .await?;
    }

    Ok(())
}
