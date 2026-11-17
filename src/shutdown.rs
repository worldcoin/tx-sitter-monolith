use tokio::signal::unix::{signal, SignalKind};
use tokio_util::sync::CancellationToken;

/// Cancels `shutdown` once the process receives SIGINT or SIGTERM.
///
/// The signal handlers are installed before this returns, so a signal that
/// arrives afterwards is never missed.
pub fn spawn_await_shutdown_task(
    shutdown: CancellationToken,
) -> eyre::Result<()> {
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;

    tokio::spawn(async move {
        tokio::select! {
            _ = sigint.recv() => { tracing::info!("SIGINT received, shutting down"); }
            _ = sigterm.recv() => { tracing::info!("SIGTERM received, shutting down"); }
        };

        shutdown.cancel();
    });

    Ok(())
}
