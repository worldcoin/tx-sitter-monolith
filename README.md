# Tx Sitter Monolith

> ⚠️ **Internal Service — Do Not Expose to the Internet**

**Note that this software is still under development. Use at your own discretion**

An easy to run transaction relayer.

## Quickstart

Copy `.env.example` to `.env` or set `RUST_LOG=info,service=debug` to enable logging.

1. Spin up the database `docker run --rm -e POSTGRES_HOST_AUTH_METHOD=trust -p 5432:5432 postgres`
2. Spin up a chain `anvil --chain-id 31337 --block-time 2`
3. Start the service `cargo run`
4. Visit <http://localhost:3000/swagger> or <http://localhost:3000/rapidoc> to interact with the api. Redoc ui is also available at <http://localhost:3000/redoc> but it's not interactive.

API schema can be downloaded from <http://localhost:3000/schema.json> or <http://localhost:3000/schema.yml>

This will use the `config.toml` configuration.

### Error reporting & debugging

For a better local development experience the `.env.example` enables color-eyre reporting.

But that by default doesn't include the backtrace or code snippets. In order to enable snippets run with `RUST_LIB_BACKTRACE=full`.

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
    TX_SITTER__SERVICE__MAX_ESCALATIONS="100"
    TX_SITTER__SERVER__HOST="127.0.0.1:3000"
    TX_SITTER__SERVER__DISABLE_AUTH="true"
    TX_SITTER__DATABASE__CONNECTION_STRING="postgres://postgres:postgres@127.0.0.1:5432/database"
    TX_SITTER__KEYS__KIND="local"
    ```

### Roles, probes and shutdown

`service.role` (`TX_SITTER__SERVICE__ROLE`) selects what a process runs:

- `all` (default): the HTTP API and the background tasks.
- `api`: the HTTP API only.
- `worker`: the background tasks, with only the health endpoints over HTTP.

Run exactly one `worker` or `all` process per database, next to any number of `api` processes.

Probes:

- `GET /health/ready`: 503 while the database doesn't answer within 2s.
- `GET /health/live` (alias `GET /health`): 503 once a background task has stopped, or has made no progress for `service.task_stall_timeout` (default `5m`).

On SIGTERM or SIGINT the service stops accepting connections and gives in-flight requests and background tasks `server.shutdown_timeout` (default `20s`) to finish.

### Relayer keys used elsewhere

A relayer's key must only send transactions through tx-sitter. When the chain's nonce for the key moves past the relayer's own, tx-sitter moves the relayer's nonce up to the chain's and logs an error. Transactions that hold a nonce the chain already used can never be mined: they are no longer broadcast or escalated, and the `relayer_stale_txs` gauge counts them per relayer.

To recover, reset the relayer with `POST /1/admin/relayer/{relayer_id}/reset`. That deletes its unsent transactions, so their callers have to send them again. Sent ones stay pending.

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

## Client

Client crate is located in `creates/tx-sitter-client`. It is generated using official OpenAPI generator with modified template files. Modified template files are located in `client-template/` directory.  Possible files to overwrite could be fined here <https://github.com/OpenAPITools/openapi-generator/tree/master/modules/openapi-generator/src/main/resources/rust>.

### Runnin script

Just run `./generate_api_client.sh`.

### Manual generation

To generate client OpenAPI spec schema is required. To get one just run api spec generator command:

```shell
cargo run --bin api_spec_generator > schema.yaml
```

Client generation is done by using default OpenAPI tools. You can install generator or use docker image as shown below:

```shell
docker run --rm -v "${PWD}:/local" --user "$(id -u):$(id -g)" -- openapitools/openapi-generator-cli generate \
  -i /local/schema.yaml \
  -g rust \
  -o /local/crates/tx-sitter-client \
  -t /local/client-template \
  --skip-validate-spec \
  --additional-properties=packageName=tx-sitter-client,supportMiddleware=true,useSingleRequestParameter=true,avoidBoxedModels=true \
  --type-mappings=address=base_api_types::Address,decimal-u256=base_api_types::DecimalU256,h256=base_api_types::H256,bytes=base_api_types::HexBytes,hex-u256=base_api_types::HexU256
```
