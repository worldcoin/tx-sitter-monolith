use clap::Parser;
use tx_sitter::server::generate_spec_yaml;

#[derive(Parser)]
#[clap(rename_all = "kebab-case")]
struct Args {
    #[clap(short, long)]
    #[cfg_attr(
        feature = "default-config",
        clap(default_value = "http://localhost:8000")
    )]
    service_endpoint: String,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let args = Args::parse();

    println!("{}", generate_spec_yaml(Some(&args.service_endpoint)).await);

    Ok(())
}
