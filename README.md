# Tx Sitter Monolith

**Note that this software is still under development. Use at your own discretion**

An easy to run transaction relayer.

## Quickstart

Copy `.env.example` to `.env` or set `RUST_LOG=info,service=debug` to have logging.

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
