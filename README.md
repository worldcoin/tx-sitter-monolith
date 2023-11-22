# Tx Sitter Monolith

A monolithized version of the [tx-sitter](https://github.com/worldcoin/tx-sitter-aws/).

## Testing locally

Copy `.env.example` to `.env` or set `RUST_LOG=info,service=debug` to have logging.

1. Spin up the database `docker run --rm -e POSTGRES_HOST_AUTH_METHOD=trust -p 5432:5432 postgres`
2. Spin up the chain `anvil --chain-id 31337 --block-time 2`
3. Start the service `cargo run`

This will use the `config.toml` configuration.

If you have [nushell](https://www.nushell.sh/) installed, `nu manual_test.nu` can be run to execute a basic test.

## Running tests
While you obviously can run tests with `cargo test --workspace` some tests take quite a long time (due to spinning up an anvil node, sending txs, etc.).

Therefore I recommend [cargo-nextest](https://nexte.st/) as it runs all the tests in parallel. Once installed `cargo nextest run --workspace` can be used instead.
