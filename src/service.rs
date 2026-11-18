use std::net::SocketAddr;
use std::sync::Arc;

use eyre::Context;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::app::App;
use crate::config::Config;
use crate::task_runner::TaskRunner;
use crate::tx_events::{self, EventNotifier};
use crate::{static_config, tasks};

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
    /// Before that it validates the config, checks the relayer keys and
    /// writes the networks, relayers and API keys into the database, see
    /// [`static_config::apply`]. A role that runs the background tasks syncs
    /// the relayers' nonces from their chains before it starts them, see
    /// [`tasks::index::sync_relayer_nonces_at_startup`]. The service runs
    /// until `shutdown` is cancelled, see [`Service::wait`].
    pub async fn new(
        config: Config,
        shutdown: CancellationToken,
    ) -> eyre::Result<Self> {
        config.validate()?;
        let app = Arc::new(App::new(config).await?);

        static_config::apply(&app).await?;

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
            tasks::index::sync_relayer_nonces_at_startup(&app).await;

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

    for network in &app.config.networks {
        let chain_id = network.chain_id;

        task_runner
            .add_task(format!("index_blocks/{chain_id}"), move |app, ctx| {
                tasks::index::index_chain(app, ctx, chain_id)
            });
        task_runner
            .add_task(format!("estimate_fees/{chain_id}"), move |app, ctx| {
                tasks::index::estimate_gas(app, ctx, chain_id)
            });

        if network.op_stack.is_some() {
            task_runner.add_task(
                format!("read_l1_fees/{chain_id}"),
                move |app, ctx| {
                    tasks::index::read_l1_fees_task(app, ctx, chain_id)
                },
            );
        }
    }
}
