use clap::Parser;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, Parser)]
struct Args {
    #[clap(short, long, default_value = "8545")]
    port: u16,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    dotenv::dotenv().ok();

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().pretty().compact())
        .with(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    let (_app, server) = fake_rpc::serve(args.port).await;

    tracing::info!("Serving fake RPC at {}", server.local_addr());

    server.await?;

    Ok(())
}
