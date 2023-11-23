use std::net::SocketAddr;
use std::sync::Arc;

use tokio::task::JoinHandle;

use crate::app::App;
use crate::config::Config;
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

        let task_runner = TaskRunner::new(app.clone());
        task_runner.add_task("Broadcast transactions", tasks::broadcast_txs);
        task_runner.add_task("Index transactions", tasks::index_blocks);
        task_runner.add_task("Escalate transactions", tasks::escalate_txs);
        task_runner.add_task("Prune blocks", tasks::prune_blocks);

        let server = crate::server::spawn_server(app.clone()).await?;
        let local_addr = server.local_addr();
        let server_handle = tokio::spawn(async move {
            server.await?;
            Ok(())
        });

        Ok(Self {
            _app: app,
            local_addr,
            server_handle,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn wait(self) -> eyre::Result<()> {
        self.server_handle.await??;

        Ok(())
    }
}
