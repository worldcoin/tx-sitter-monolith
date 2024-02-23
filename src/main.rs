use std::path::PathBuf;

use clap::Parser;
use telemetry_batteries::metrics::statsd::StatsdBattery;
use telemetry_batteries::tracing::datadog::DatadogBattery;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;
use tx_sitter::config::load_config;
use tx_sitter::service::Service;

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
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let args = Args::parse();

    dotenv::dotenv().ok();

    for path in &args.env_file {
        dotenv::from_path(path)?;
    }

    let config = load_config(args.config.iter().map(PathBuf::as_ref))?;

    if config.service.datadog_enabled {
        DatadogBattery::init(None, "tx-sitter-monolith", None, true);
    } else {
        tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer().pretty().compact())
            .with(EnvFilter::from_default_env())
            .init();
    }

    if config.service.statsd_enabled {
        StatsdBattery::init(
            "localhost",
            8125,
            5000,
            1024,
            Some("tx_sitter_monolith"),
        )?;
    }

    tracing::info!(?config, "Starting service");
    let service = Service::new(config).await?;
    service.wait().await?;

    Ok(())
}
