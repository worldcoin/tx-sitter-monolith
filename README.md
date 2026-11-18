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
- `api`: the HTTP API only, and the listener for its event feed.
- `worker`: the background tasks, with only the health endpoints over HTTP.

Run exactly one `worker` or `all` process per database, next to any number of `api` processes.

Probes:

- `GET /health/ready`: 503 while the database doesn't answer within 2s.
- `GET /health/live` (alias `GET /health`): 503 once a background task has stopped, or has made no progress for `service.task_stall_timeout` (default `5m`).

Every database statement times out after 30s, including the time it waits for locks, so a background task blocked on the database fails and restarts with a backoff rather than stalling. Migrations run without that timeout. The service sets it with the `options` startup parameter and refuses to start if a connection doesn't have it, as behind a PgBouncer that lists `options` in `ignore_startup_parameters`.

On SIGTERM or SIGINT the service stops accepting connections and gives in-flight requests and background tasks `server.shutdown_timeout` (default `20s`) to finish.

### Relayer keys used elsewhere

A relayer's key must only send transactions through tx-sitter. When the chain's nonce for the key moves past the relayer's own, tx-sitter moves the relayer's nonce up to the chain's and logs an error. Transactions that hold a nonce the chain already used can never be mined: they are no longer broadcast or escalated, and the `relayer_stale_txs` gauge counts them per relayer.

To recover, reset the relayer with `POST /1/admin/relayer/{relayer_id}/reset`. That deletes its unsent transactions, so their callers have to send them again. Sent ones stay pending.

### Relayer API v2

`POST /2/tx`, `GET /2/tx/{txId}` and `GET /2/events` take the relayer's API key in an `Authorization: Bearer <api key>` header, never in the URL. The v1 API under `/1/api/<api key>/` is unchanged.

A send may carry `metadata`, a JSON object of at most 64 KiB that tx-sitter stores and returns byte for byte. Sending a `txId` again with an identical request, metadata text included, returns the existing transaction without taking a nonce. A different request with the same `txId` returns 409. That holds while the transaction is kept: finalized transactions are deleted `service.tx_retention` (default `30d`) after they are mined.

Every v2 error is a JSON `{ "errorId": ..., "errorMessage": ... }` whose `errorId` stays stable. The OpenAPI schema lists them per status.

### Transaction events

`GET /2/events` returns the status changes of the relayer's transactions in order: `pending` when a transaction is broadcast, escalated or reorged out of its block, `mined` when it is in a block or reorged into another one, and `finalized`. Each event carries the transaction's id, hash and `metadata`. A consumer keeps the response's `nextCursor` and passes it back as `after`, so it gets every event once, in order, also after a restart. `wait=<seconds>` (at most 30) waits for an event when there is none.

- Events are stored in the database transaction that makes the change. The feed returns an event only once every database transaction that started writing before it has finished, so that no event is stored behind one already returned. A transaction that stays open holds the feed back; Postgres closes tx-sitter's own connections after 30s idle in a transaction, and the worker's `tx_events_held_back_seconds` gauge shows how long the feed has been held back.
- Events are deleted `service.event_retention` (default `7d`) after they are stored. A cursor from before deleted events gets 410 `cursor_expired`: the consumer may have missed events and has to restart without a cursor.
- Every process that serves the API listens for new events on one database connection. At most `server.max_event_waiters` (default `64`) requests wait at once per process; further ones get 503 `too_many_waiters` with `Retry-After`.
- `notification_queue_usage` is the fraction of Postgres' notification queue in use. Storing events, and with them every status change, fails once it is full.

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
