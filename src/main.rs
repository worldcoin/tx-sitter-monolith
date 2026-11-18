use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use eyre::{Context, ContextCompat};
use telemetry_batteries::metrics::statsd::StatsdBattery;
use telemetry_batteries::tracing::datadog::DatadogBattery;
use telemetry_batteries::tracing::TracingShutdownHandle;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tx_sitter::api_key::ApiKey;
use tx_sitter::config::{load_config, Config};
use tx_sitter::config_dump;
use tx_sitter::db::Database;
use tx_sitter::service::Service;
use tx_sitter::shutdown::spawn_await_shutdown_task;

const TRACING_FLUSH_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Parser)]
#[command(author, version, about)]
#[clap(rename_all = "kebab-case")]
struct Args {
    #[clap(short, long, global = true)]
    #[cfg_attr(
        feature = "default-config",
        clap(default_value = "config.toml")
    )]
    config: Vec<PathBuf>,

    #[clap(short, long, global = true)]
    env_file: Vec<PathBuf>,

    /// Installs color-eyre hooks for better messages
    ///
    /// Useful for local testing and debugging
    /// Not very useful for production deployments
    #[clap(short = 'E', long, env, global = true)]
    color_eyre: bool,

    /// What to do, `serve` by default
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Runs the service
    Serve,

    /// Works with the networks, relayers and API keys of the config
    #[command(subcommand)]
    Config(ConfigCommand),

    /// Works with API keys
    #[command(subcommand)]
    ApiKey(ApiKeyCommand),

    /// Works with the relayers in the database
    #[command(subcommand)]
    Relayer(RelayerCommand),
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Prints the networks, relayers and API key hashes in the database as
    /// config, to move a database of the admin API to the static config once
    ///
    /// It only reads the database. RPC URLs and local private keys are
    /// printed as `${NAME}` environment variable references, and their values
    /// are listed on stderr. The database holds one HTTP RPC and the WS RPC
    /// of each network, so only those are exported.
    ///
    /// The output holds only `[[networks]]` and `[[relayers]]`: load it after
    /// the service's config, with `--config config.toml --config dump.toml`.
    Dump,
}

#[derive(Subcommand)]
enum ApiKeyCommand {
    /// Prints a new API key for a relayer of the config, and the hash to add
    /// to its `api_key_hashes`
    New {
        #[clap(long)]
        relayer_id: uuid::Uuid,
    },
}

#[derive(Subcommand)]
enum RelayerCommand {
    /// Deletes the relayer's unsent transactions and rewinds its nonce, to
    /// unstick it
    Reset {
        #[clap(long)]
        relayer_id: uuid::Uuid,
    },
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

    let config_files = args.config;
    let load_config = || load_config(config_files.iter().map(PathBuf::as_ref));

    match args.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(load_config()?).await,
        Command::Config(ConfigCommand::Dump) => {
            init_cli_tracing();
            dump_config(load_config()?).await
        }
        Command::ApiKey(ApiKeyCommand::New { relayer_id }) => {
            print_new_api_key(load_config()?, relayer_id)
        }
        Command::Relayer(RelayerCommand::Reset { relayer_id }) => {
            init_cli_tracing();
            reset_relayer(load_config()?, relayer_id).await
        }
    }
}

/// Logs to stderr, so that it doesn't mix with a command's output.
fn init_cli_tracing() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
}

async fn serve(config: Config) -> eyre::Result<()> {
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

fn print_new_api_key(
    config: Config,
    relayer_id: uuid::Uuid,
) -> eyre::Result<()> {
    let relayer_id = relayer_id.to_hyphenated().to_string();
    if !config
        .relayers
        .iter()
        .any(|relayer| relayer.id == relayer_id)
    {
        eyre::bail!("Relayer {relayer_id} is not in the config");
    }

    let api_key = ApiKey::random(relayer_id);

    println!("api_key = \"{}\"", api_key.reveal()?);
    println!(
        "api_key_hash = \"0x{}\"",
        hex::encode(api_key.api_key_secret_hash())
    );
    eprintln!(
        "Give the API key to its caller, it can't be recovered. Add its hash \
         to the relayer's api_key_hashes."
    );

    Ok(())
}

async fn dump_config(config: Config) -> eyre::Result<()> {
    let db = Database::connect_read_only(&config.database).await?;

    let dump = config_dump::dump(&db, &config.keys).await?;

    print!("{}", dump.toml);
    eprintln!(
        "WARNING: this is a one-time export of a database that the admin API \
         managed. It has one HTTP RPC and the WS RPC of each network, add \
         fallback RPCs to http_rpcs by hand. Load it after the service's \
         config: --config config.toml --config dump.toml"
    );
    if !dump.secrets.is_empty() {
        eprintln!(
            "WARNING: these environment variables hold secrets, store them \
             in the secrets manager and never in the config:"
        );
    }
    for (name, value) in dump.secrets {
        eprintln!("{name}={value}");
    }

    Ok(())
}

async fn reset_relayer(
    config: Config,
    relayer_id: uuid::Uuid,
) -> eyre::Result<()> {
    let relayer_id = relayer_id.to_hyphenated().to_string();
    let db = Database::connect_current(&config.database).await?;

    db.get_relayer(&relayer_id)
        .await?
        .with_context(|| format!("Relayer {relayer_id} not found"))?;
    db.purge_unsent_txs(&relayer_id)
        .await
        .with_context(|| format!("Resetting relayer {relayer_id}"))?;

    eprintln!("Reset relayer {relayer_id}");

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
