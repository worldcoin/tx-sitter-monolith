# Tx Sitter Monolith

A monolithized version of the [tx-sitter](https://github.com/worldcoin/tx-sitter-aws/).

## Configuration
The Tx Sitter can be configured in 2 ways:
1. Using the config file, refer to `config.rs` and `config.toml` for more info
2. Using env vars. Every field in the config can also be set via an env var.
   For example the following config
   ```toml
    [service]
    escalation_interval = "1m"

    [server]
    host = "127.0.0.1:3000"
    disable_auth = true

    [database]
    connection_string = "postgres://postgres:postgres@127.0.0.1:5432/database"

    [keys]
    kind = "local"
    ```

    Can also be expressed with env vars
    ```
    TX_SITTER__SERVICE__ESCALATION_INTERVAL="1m"
    TX_SITTER__SERVER__HOST="127.0.0.1:3000"
    TX_SITTER__SERVER__DISABLE_AUTH="true"
    TX_SITTER__DATABASE__CONNECTION_STRING="postgres://postgres:postgres@127.0.0.1:5432/database"
    TX_SITTER__KEYS__KIND="local"
    ```

## Testing locally
Copy `.env.example` to `.env` or set `RUST_LOG=info,service=debug` to have logging.

1. Spin up the database `docker run --rm -e POSTGRES_HOST_AUTH_METHOD=trust -p 5432:5432 postgres`
2. Spin up the chain `anvil --chain-id 31337 --block-time 2`
3. Start the service `cargo run`

This will use the `config.toml` configuration.

If you have [nushell](https://www.nushell.sh/) installed, `nu manual_test.nu` can be run to execute a basic test.

## Running tests
While you obviously can run tests with
```
cargo test --workspace
```
some tests take quite a long time (due to spinning up an anvil node, sending txs, etc.).

Therefore I recommend [cargo-nextest](https://nexte.st/) as it runs all the tests in parallel. Once installed
```
cargo nextest run --workspace
```
can be used instead.
