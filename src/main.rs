use std::path::PathBuf;

use clap::Parser;
use config::FileFormat;
use service::config::Config;
use service::service::Service;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[clap(rename_all = "kebab-case")]
struct Args {
    #[clap(short, long, default_value = "./config.toml")]
    config: PathBuf,

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

    let settings = config::Config::builder()
        .add_source(
            config::File::from(args.config.as_ref()).format(FileFormat::Toml),
        )
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
