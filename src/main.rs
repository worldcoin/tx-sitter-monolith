use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use telemetry_batteries::metrics::statsd::StatsdBattery;
use telemetry_batteries::tracing::datadog::DatadogBattery;
use telemetry_batteries::tracing::TracingShutdownHandle;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tx_sitter::config::{load_config, Config};
use tx_sitter::service::Service;
use tx_sitter::shutdown::spawn_await_shutdown_task;

const TRACING_FLUSH_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Parser)]
#[command(author, version, about)]
#[clap(rename_all = "kebab-case")]
struct Args {
    #[clap(short, long)]
    #[cfg_attr(
        feature = "default-config",
        clap(default_value = "config.toml")
    )]
    config: Vec<PathBuf>,

    #[clap(short, long)]
    env_file: Vec<PathBuf>,

    /// Installs color-eyre hooks for better messages
    ///
    /// Useful for local testing and debugging
    /// Not very useful for production deployments
    #[clap(short = 'E', long, env)]
    color_eyre: bool,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let args = Args::parse();

    dotenv::dotenv().ok();
    for path in &args.env_file {
        dotenv::from_path(path)?;
    }

    // Reparse the args to account for newly loaded env vars
    let args = Args::parse();

    if args.color_eyre {
        color_eyre::install()?;
    }

    let config = load_config(args.config.iter().map(PathBuf::as_ref))?;

    let tracing_shutdown_handle =
        if let Some(telemetry) = &config.service.telemetry {
            let tracing_shutdown_handle = DatadogBattery::init(
                telemetry.traces_endpoint.as_deref(),
                &telemetry.service_name,
                None,
                true,
            );

            if let Some(metrics_config) = &telemetry.metrics {
                StatsdBattery::init(
                    &metrics_config.host,
                    metrics_config.port,
                    metrics_config.queue_size,
                    metrics_config.buffer_size,
                    Some(&metrics_config.prefix),
                )?;
            }

            tracing_shutdown_handle
        } else {
            tracing_subscriber::registry()
                .with(tracing_subscriber::fmt::layer().pretty().compact())
                .with(tracing_subscriber::EnvFilter::from_default_env())
                .with(tracing_error::ErrorLayer::default())
                .init();

            TracingShutdownHandle
        };

    let result = run(config).await;
    flush_tracing(tracing_shutdown_handle);

    result
}

async fn run(config: Config) -> eyre::Result<()> {
    let shutdown = CancellationToken::new();
    spawn_await_shutdown_task(shutdown.clone())?;

    tracing::info!(?config, "Starting service");
    let service = tokio::select! {
        service = Service::new(config, shutdown.clone()) => service?,
        () = shutdown.cancelled() => {
            tracing::warn!("Shutdown requested during startup, exiting before the service started");
            return Ok(());
        }
    };
    service.wait().await?;

    tracing::info!("Shutdown complete");

    Ok(())
}

/// Exports the remaining traces, giving up after [`TRACING_FLUSH_TIMEOUT`] so
/// that an unreachable trace agent can't delay the exit.
fn flush_tracing(handle: TracingShutdownHandle) {
    let (flushed_tx, flushed_rx) = std::sync::mpsc::channel();

    // A plain thread, because the runtime waits for `spawn_blocking` tasks
    // when it shuts down
    std::thread::spawn(move || {
        drop(handle);
        flushed_tx.send(()).ok();
    });

    if flushed_rx.recv_timeout(TRACING_FLUSH_TIMEOUT).is_err() {
        tracing::warn!(
            timeout = ?TRACING_FLUSH_TIMEOUT,
            "Traces were not exported within the timeout, dropping them"
        );
    }
}
