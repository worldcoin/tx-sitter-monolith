# tx-sitter-service (proper name pending)

## Testing locally

Copy `.env.example` to `.env` or set `RUST_LOG=info,service=debug` to have logging.

1. Spin up the database `docker run --rm -e POSTGRES_HOST_AUTH_METHOD=trust -p 5432:5432 postgres`
2. Spin up the chain `anvil --chain-id 31337 --block-time 2`
3. Start the service `cargo run`

This will use the `config.toml` configuration.
