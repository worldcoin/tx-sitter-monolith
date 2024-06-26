use std::path::PathBuf;

use clap::Parser;
use telemetry_batteries::metrics::statsd::StatsdBattery;
use telemetry_batteries::tracing::datadog::DatadogBattery;
use telemetry_batteries::tracing::TracingShutdownHandle;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tx_sitter::config::load_config;
use tx_sitter::service::Service;
use tx_sitter::shutdown::spawn_await_shutdown_task;

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

    let _tracing_shutdown_handle =
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

    spawn_await_shutdown_task();

    tracing::info!(?config, "Starting service");
    let service = Service::new(config).await?;
    service.wait().await?;

    Ok(())
}
