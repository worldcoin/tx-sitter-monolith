use std::path::{Path, PathBuf};

use tx_sitter::config::load_config;

/// Loads `config.toml` and the config of `compose.yml`, its environment and
/// `compose.config.toml`, as the service would.
///
/// Both run in one test, because the compose config is partly environment
/// variables, which every test in a process shares.
#[test]
fn shipped_configs_load_and_validate() -> eyre::Result<()> {
    let config =
        load_config(std::iter::once(repo_file("config.toml").as_path()))?;
    config.validate()?;

    for (name, value) in compose_environment(&repo_file("compose.yml"))? {
        std::env::set_var(name, value);
    }
    let config = load_config(std::iter::once(
        repo_file("compose.config.toml").as_path(),
    ))?;
    config.validate()?;

    assert_eq!(config.server.host, "0.0.0.0:3000".parse()?);
    assert_eq!(config.relayers.len(), 1);

    Ok(())
}

fn repo_file(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(name)
}

/// Returns the `TX_SITTER*` environment variables that `compose.yml` sets.
fn compose_environment(path: &Path) -> eyre::Result<Vec<(String, String)>> {
    let compose = std::fs::read_to_string(path)?;

    compose
        .lines()
        .filter_map(|line| line.trim().strip_prefix("- TX_SITTER"))
        .map(|variable| {
            let (name, value) = variable.split_once('=').ok_or_else(|| {
                eyre::eyre!("No value in TX_SITTER{variable}")
            })?;

            Ok((format!("TX_SITTER{name}"), value.to_string()))
        })
        .collect()
}
