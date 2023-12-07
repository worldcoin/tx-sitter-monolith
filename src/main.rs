use std::path::PathBuf;

use clap::Parser;
use config::FileFormat;
use telemetry_batteries::metrics::statsd::StatsdBattery;
use telemetry_batteries::metrics::MetricsBattery;
use telemetry_batteries::tracing::batteries::datadog::DatadogBattery;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;
use tx_sitter::config::Config;
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

    let mut settings = config::Config::builder();

    for arg in &args.config {
        settings = settings.add_source(
            config::File::from(arg.as_ref()).format(FileFormat::Toml),
        );
    }

    let settings = settings
        .add_source(
            config::Environment::with_prefix("TX_SITTER").separator("__"),
        )
        .add_source(
            config::Environment::with_prefix("TX_SITTER_EXT")
                .separator("__")
                .try_parsing(true)
                .list_separator(","),
        )
        .build()?;

    let config = settings.try_deserialize::<Config>()?;

    if config.service.datadog_enabled {
        let datadog_battery =
            DatadogBattery::new(None, "tx-sitter-monolith", None)
                .with_location();

        datadog_battery.init()?;
    } else {
        tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer().pretty().compact())
            .with(EnvFilter::from_default_env())
            .init();
    }

    if config.service.statsd_enabled {
        let statsd_battery = StatsdBattery::new(
            "localhost",
            8125,
            5000,
            1024,
            Some("tx_sitter_monolith"),
        )?;

        statsd_battery.init()?;
    }

    let service = Service::new(config).await?;
    service.wait().await?;

    Ok(())
}
