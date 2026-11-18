use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use alloy::signers::local::PrivateKeySigner;
use eyre::Context;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::app::App;
use crate::config::Config;
use crate::keys::local_keys::signing_key_from_hex;
use crate::task_runner::{TaskContext, TaskRunner};
use crate::tasks;
use crate::tx_events::{self, EventNotifier};

const NETWORK_RECONCILIATION_INTERVAL: Duration = Duration::from_secs(30);

pub struct Service {
    app: Arc<App>,
    task_runner: Arc<TaskRunner<App>>,
    shutdown: CancellationToken,
    local_addr: SocketAddr,
    server_handle: JoinHandle<eyre::Result<()>>,
}

impl Service {
    /// Starts the parts of the service that `config.service.role` selects.
    ///
    /// The service runs until `shutdown` is cancelled, see [`Service::wait`].
    pub async fn new(
        config: Config,
        shutdown: CancellationToken,
    ) -> eyre::Result<Self> {
        config.validate()?;
        let app = Arc::new(App::new(config).await?);

        initialize_predefined_values(&app).await?;

        let task_runner = Arc::new(TaskRunner::new(
            app.clone(),
            shutdown.clone(),
            app.config.service.task_stall_timeout,
        ));

        let event_notifier = Arc::new(EventNotifier::new(
            app.config.server.max_event_waiters,
            shutdown.clone(),
        ));
        let server = crate::server::spawn_server(
            app.clone(),
            task_runner.clone(),
            event_notifier.clone(),
            shutdown.clone(),
        )
        .await?;

        if app.config.service.role.serves_api() {
            task_runner.add_task("listen_tx_events", move |app, ctx| {
                tx_events::listen_for_tx_events(
                    app,
                    ctx,
                    event_notifier.clone(),
                )
            });
        }

        if app.config.service.role.runs_tasks() {
            tracing::info!("Spawning tasks");
            add_background_tasks(&task_runner, &app);
        }

        Ok(Self {
            app,
            task_runner,
            shutdown,
            local_addr: server.local_addr(),
            server_handle: server.server_handle,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    // Integration tests add their own tasks through this
    #[doc(hidden)]
    pub fn task_runner(&self) -> &Arc<TaskRunner<App>> {
        &self.task_runner
    }

    /// Waits for shutdown to be requested, then for in-flight requests and
    /// background tasks to finish.
    ///
    /// Both get `server.shutdown_timeout` to finish. Tasks still running after
    /// that are aborted and logged.
    pub async fn wait(self) -> eyre::Result<()> {
        self.shutdown.cancelled().await;

        let deadline = Instant::now() + self.app.config.server.shutdown_timeout;
        let (server_result, unfinished_tasks) =
            tokio::join!(self.server_handle, self.task_runner.join(deadline));

        if !unfinished_tasks.is_empty() {
            tracing::warn!(
                ?unfinished_tasks,
                shutdown_timeout = ?self.app.config.server.shutdown_timeout,
                "Tasks did not finish within the shutdown timeout"
            );
        }

        server_result.context("HTTP server panicked")?
    }

    pub async fn are_estimates_ready_for_chain(&self, chain_id: u64) -> bool {
        let res = self
            .app
            .db
            .get_latest_block_fees_by_chain_id(chain_id)
            .await;
        match res {
            Ok(res) => res.is_some(),
            Err(_) => false,
        }
    }
}

fn add_background_tasks(task_runner: &Arc<TaskRunner<App>>, app: &App) {
    task_runner.add_watchdog();
    task_runner.add_task("broadcast_txs", tasks::broadcast_txs);
    task_runner.add_task("escalate_txs", tasks::escalate_txs_task);
    task_runner.add_task("prune_blocks", tasks::prune_blocks);
    task_runner.add_task("prune_txs", tasks::prune_txs);
    task_runner.add_task("prune_tx_events", tasks::prune_tx_events);
    task_runner.add_task("finalize_txs", tasks::finalize_txs);
    task_runner.add_task("handle_soft_reorgs", tasks::handle_soft_reorgs);
    task_runner.add_task("handle_hard_reorgs", tasks::handle_hard_reorgs);
    task_runner.add_task("monitor_funds", tasks::monitor_funds);
    task_runner.add_task("monitor_nonces", tasks::monitor_nonces);

    if let Some(telemetry_config) = app.config.service.telemetry.as_ref() {
        if telemetry_config.metrics.is_some() {
            task_runner.add_task("emit_metrics", tasks::emit_metrics);
        }
    }

    let runner = task_runner.clone();
    task_runner.add_task("reconcile_networks", move |app, ctx| {
        reconcile_networks(app, ctx, runner.clone())
    });
}

/// Keeps one set of chain tasks running for every network in the database,
/// including networks added after startup.
async fn reconcile_networks(
    app: Arc<App>,
    ctx: TaskContext,
    task_runner: Arc<TaskRunner<App>>,
) -> eyre::Result<()> {
    while !ctx.is_shutting_down() {
        for chain_id in app.db.get_network_chain_ids().await? {
            task_runner.add_task(
                format!("index_blocks/{chain_id}"),
                move |app, ctx| tasks::index::index_chain(app, ctx, chain_id),
            );
            task_runner.add_task(
                format!("estimate_fees/{chain_id}"),
                move |app, ctx| tasks::index::estimate_gas(app, ctx, chain_id),
            );
        }

        ctx.idle(NETWORK_RECONCILIATION_INTERVAL).await;
    }

    Ok(())
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
    }

    let secret_key = signing_key_from_hex(&predefined.relayer.key_id)?;

    let signer = PrivateKeySigner::from(secret_key);
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
