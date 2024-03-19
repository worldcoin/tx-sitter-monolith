use core::panic;

use tokio::signal::unix::{signal, SignalKind};

pub fn spawn_await_shutdown_task() {
    tokio::spawn(async {
        let result = await_shutdown_signal().await;
        if let Err(err) = result {
            tracing::error!("Error while waiting for shutdown signal: {}", err);
            panic!("Error while waiting for shutdown signal: {}", err);
        }

        tracing::info!("Shutdown complete");
        std::process::exit(0);
    });
}

pub async fn await_shutdown_signal() -> eyre::Result<()> {
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;

    tokio::select! {
        _ = sigint.recv() => { tracing::info!("SIGINT received, shutting down"); }
        _ = sigterm.recv() => { tracing::info!("SIGTERM received, shutting down"); }
    };

    Ok(())
}
