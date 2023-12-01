use std::path::PathBuf;

use clap::Parser;
use config::FileFormat;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;
use tx_sitter::config::Config;
use tx_sitter::service::Service;

#[derive(Parser)]
#[command(author, version, about)]
#[clap(rename_all = "kebab-case")]
struct Args {
    #[clap(short, long, default_value = "config.toml")]
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

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().pretty().compact())
        .with(EnvFilter::from_default_env())
        .init();

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

    let service = Service::new(config).await?;
    service.wait().await?;

    Ok(())
}
