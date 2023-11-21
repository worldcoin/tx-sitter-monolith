use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use config::FileFormat;
use service::app::App;
use service::config::Config;
use service::task_backoff::TaskRunner;
use service::tasks;
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

    let app = Arc::new(App::new(config).await?);

    let task_runner = TaskRunner::new(app.clone());
    task_runner.add_task("Broadcast transactions", tasks::broadcast_txs);
    task_runner.add_task("Index transactions", tasks::index_blocks);
    task_runner.add_task("Escalate transactions", tasks::escalate_txs);

    service::server::serve(app).await?;

    Ok(())
}
