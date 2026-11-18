use eyre::Context;
use futures::stream::{self, StreamExt};

use crate::app::App;
use crate::config::RelayerConfig;

/// How many relayer keys are loaded at once at startup
const KEY_CHECK_CONCURRENCY: usize = 8;

/// Writes the networks, relayers and API keys of the config into the
/// database, see [`Database::apply_static_config`].
///
/// First it checks that the key of every enabled relayer derives the
/// configured address, so a config with a wrong key or address fails startup
/// before it changes the database.
///
/// [`Database::apply_static_config`]: crate::db::Database::apply_static_config
pub async fn apply(app: &App) -> eyre::Result<()> {
    verify_relayer_keys(app).await?;

    let disabled_relayers = app
        .db
        .apply_static_config(&app.config.networks, &app.config.relayers)
        .await
        .context("Writing the static config to the database")?;

    for relayer in disabled_relayers {
        tracing::warn!(
            relayer_id = relayer.id,
            relayer_name = relayer.name,
            "Disabled a relayer that is not in the config"
        );
    }

    Ok(())
}

/// Loads the key of every enabled relayer and fails, listing every relayer at
/// fault, if a key can't be loaded or derives another address than the
/// configured one.
///
/// KMS keys are loaded with `GetPublicKey`, bounded by `keys.kms_timeout`.
async fn verify_relayer_keys(app: &App) -> eyre::Result<()> {
    let errors: Vec<String> = stream::iter(&app.config.relayers)
        .filter(|relayer| std::future::ready(relayer.enabled))
        .map(|relayer| verify_relayer_key(app, relayer))
        .buffer_unordered(KEY_CHECK_CONCURRENCY)
        .filter_map(|result| std::future::ready(result.err()))
        .map(|err| format!("{err:#}"))
        .collect()
        .await;

    if !errors.is_empty() {
        eyre::bail!("Relayer key check failed:\n- {}", errors.join("\n- "));
    }

    Ok(())
}

async fn verify_relayer_key(
    app: &App,
    relayer: &RelayerConfig,
) -> eyre::Result<()> {
    let signer = app
        .keys_source
        .load_signer(relayer.key.key_id().to_string())
        .await
        .with_context(|| format!("relayer {}: loading its key", relayer.id))?;

    let derived_address = signer.address();
    if derived_address != relayer.address {
        eyre::bail!(
            "relayer {}: expected address {}, but its key derives {derived_address}",
            relayer.id,
            relayer.address
        );
    }

    Ok(())
}
