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

This will use the `config.toml` configuration, which defines a relayer on the anvil chain with the API key `G5CKNF3BTS2hRl60bpdYMNPqXvXsP-QZd2lrtmgctsk=`.

### Error reporting & debugging

For a better local development experience the `.env.example` enables color-eyre reporting.

But that by default doesn't include the backtrace or code snippets. In order to enable snippets run with `RUST_LIB_BACKTRACE=full`.

## Configuration

The Tx Sitter can be configured in 2 ways:

1. Using the config file, refer to `config.rs` and `config.toml` for more info
2. Using env vars. Every field in the config, except the `[[networks]]` and `[[relayers]]` lists, can also be set via an env var.
   For example the following config

   ```toml
    [service]
    escalation_interval = "1m"

    [server]
    host = "127.0.0.1:3000"

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
    TX_SITTER__DATABASE__CONNECTION_STRING="postgres://postgres:postgres@127.0.0.1:5432/database"
    TX_SITTER__KEYS__KIND="local"
    ```

A string value in a config file can reference environment variables as `${NAME}`. Use this for secrets such as RPC URLs, which usually embed a provider API key. A reference to an unset variable fails startup. `${` always starts a reference: there is no way to write a literal `${`.

### Networks, relayers and API keys

Networks, relayers and API keys are defined in the config, not through an API. Every process applies them to the database when it starts, so the database only holds runtime state such as nonces and transactions.

```toml
[service]
# Raised by every deploy, see "Config revisions"
config_revision = 1731801600

[[networks]]
chain_id = 480
name = "worldchain"
# In order of preference: a request moves on to the next RPC when one can't be reached, times out, or answers with HTTP 5xx or 429
http_rpcs = ["${WORLDCHAIN_RPC_URL}", "${WORLDCHAIN_RPC_URL_FALLBACK}"]
ws_rpc = "${WORLDCHAIN_WS_URL}"

# OP-stack chains only, see "L1 fee gating"
[networks.op_stack]
max_l1_base_fee = "100000000000" # in wei
max_blob_base_fee = "400000000000" # in wei
max_fee_age = "2m" # default 2m

[[relayers]]
# A lowercase UUID. API keys embed it, so it never changes.
id = "6b0b6d1c-7b0e-4c2a-9d3e-2f1a5c4b3a29"
name = "wc_enableSafe4337Module_1"
chain_id = 480
# A KMS key id or key ARN with keys.kind = "kms", or { local = "<hex private key>" } with keys.kind = "local".
# A key of another AWS account needs its key ARN. Aliases are refused, because they can be pointed at another key.
key = { kms = "arn:aws:kms:eu-central-1:123456789012:key/0b3e2a5c-..." }
# The address the key must derive, checked at startup
address = "0x1Ed53d680B8890DAe2a63f673a85fFDE1FD5C7a2"
max_inflight_txs = 5 # default 5
max_queued_txs = 20 # default 20, must be greater than max_inflight_txs
# Broadcasting pauses while the chain's gas price is above a limit
gas_price_limits = [{ chain_id = 480, value = "100000000000" }] # default []
enabled = true # default true
# Hashes of the API keys the relayer accepts
api_key_hashes = ["0x235704acd89680dcf5be2b7ddbead1f01e93c200f8a769afad104fde70d9ec7d"] # default []
```

At startup every process:

1. Validates the config: distinct chain and relayer ids, distinct addresses on each chain, every relayer on a configured network, at least one HTTP RPC per network, keys of the kind `keys.kind` selects. Every problem is reported at once.
2. Compares `service.config_revision` with the revision the database applied last, see "Config revisions". If the database already applied this revision and config, the process starts without the next two steps.
3. Loads the key of every enabled relayer, with KMS `GetPublicKey` for KMS keys, and fails if it derives another address than `address`. Timeouts, throttling and KMS internal errors are retried for up to 60s.
4. Writes the config into the database in one transaction, and logs every change at warn:
   - networks and relayers are upserted, and relayers keep their nonces;
   - a relayer's `chain_id` and `address` can change only until it has transactions or allocated a nonce, after that add a new relayer instead;
   - an enabled relayer can't have the chain id and address of another relayer that is enabled or has unmined transactions, because both would use the account's nonces;
   - a relayer that is in the database but not in the config is disabled, not deleted, and loses its API keys;
   - every configured relayer accepts exactly the API keys in its `api_key_hashes`;
   - a config without relayers fails while the database has enabled relayers, which protects a database whose relayers were never moved to the config.
5. With `service.role` `all` or `worker`, syncs every relayer's nonce from its chain before it starts the background tasks. A chain whose RPCs fail is logged and left to indexing, which syncs nonces with every block.

A relayer whose nonce isn't synced answers sends with 503 (`relayer_not_ready`) and allocates no nonce: a new relayer, one that was enabled again, and one whose `chain_id` or `address` changed. Only an enabled relayer syncs. Its first sync moves its nonce up to the chain's, so a key that already sent transactions elsewhere continues from there, and is refused while the address has pending transactions above both nonces, which another signer still sends.

A disabled relayer answers every request with 403, including reads, the event feed and the RPC proxy, and broadcasts nothing. Only a v2 send that repeats a stored one still gets its `txId`, as the v2 API promises. Its unsent and in-flight transactions are logged at warn when a process applies the config.

The service no longer creates KMS keys, so it needs only `kms:GetPublicKey` and `kms:Sign` on the relayer keys.

### L1 fee gating

A transaction on an OP-stack chain such as Optimism or Worldchain also pays for posting its data to L1. On a network with an `op_stack` section, broadcasting and escalation pause while the chain's L1 base fee is above `max_l1_base_fee`, or its blob base fee is above `max_blob_base_fee`.

- **Source.** Every `service.fee_estimation_interval` (default `30s`) the worker reads both fees from the chain's own `GasPriceOracle` predeploy, `0x420000000000000000000000000000000000000F`, through the network's HTTP RPCs, and stores them with the fee estimate.
- **Failed reads.** A failed read is logged at warn (`Failed to read L1 fees`) and counted in `l1_fee_read_failures`. The fees last read stay in effect for `max_fee_age`.
- **Unknown fees hold transactions.** Until the fees are first read, and once the last successful read is older than `max_fee_age`, transactions are held as if a fee were above its cap. `max_fee_age` must be greater than `service.fee_estimation_interval`.
- **Logs.** The gate logs at warn when it starts holding transactions (`Holding transactions because of L1 fees`, with the reason and the fees) and when it stops, not on every check.
- **Metrics.** `l1_base_fee` and `blob_base_fee` gauges in gwei, and `l1_fee_held_txs`, the chain's unsent transactions while the gate holds them. All are tagged with `chain_id`.

Networks without `op_stack` never read the oracle.

### Config revisions

Every process applies its own config when it starts, so without a check a process with an old config, restarting or scaling up after a rollout, would undo the new config for every process. `service.config_revision` (`TX_SITTER__SERVICE__CONFIG_REVISION`) prevents that. The database stores the revision and a hash of the networks and relayers it applied last, and a starting process:

- applies a higher revision;
- does nothing for the same revision and hash;
- refuses to start with a lower revision, or with the same revision and another config.

The hash leaves out RPC URLs, so rotating an RPC secret doesn't need a new revision. Every process sends requests to the RPCs of its own config.

The deploy passes the deploy run's timestamp as the revision, the same one to the API and worker releases. To roll back, run the deploy workflow again with the old version: the old config gets a new, higher revision. Restarting old pods doesn't roll back, they refuse to start.

Without `config_revision`, for local development, the config is applied whatever the database applied before, and the stored revision is forgotten.

### Operating guide

Every change below is a config change followed by a deploy.

- **Add a network.** Add a `[[networks]]` entry, and put its RPC URLs in the secrets manager as the environment variables it references.
- **Add a relayer.** Create a secp256k1 signing key in KMS (`ECC_SECG_P256K1`, `SIGN_VERIFY`), derive its address, and add a `[[relayers]]` entry with a new UUID (`uuidgen | tr A-Z a-z`), the key and the address. Fund the address.
- **Add or rotate an API key.** Run `tx-sitter --config <the service's config> api-key new --relayer-id <id>`, add the printed hash to the relayer's `api_key_hashes` and give the printed key to the caller. The key can't be recovered later. To revoke a key, remove its hash.
- **Disable a relayer.** Set `enabled = false`, or remove the relayer from the config, which also revokes its API keys.
- **Reset a stuck relayer.** Run `tx-sitter --config <the service's config> relayer reset --relayer-id <id>`. It deletes the relayer's unsent transactions and moves its nonce back to the first one no remaining transaction holds, never below a nonce the chain or a sent transaction used. It needs the database schema of its own version, so run the image that is deployed.

### Import a key from tx-sitter-aws

A tx-sitter-aws relayer signs with a KMS key in a tx-sitter-aws account, and contracts whitelist its address, so it moves to this service with its key. A key must never sign in both services at once, because both would use the same nonces. The service can't prevent that, so hand a key over only once tx-sitter-aws has stopped using it.

Before the first import:

- **Run this version everywhere.** Import a key only after this image runs on every API and worker pod, never in the deploy that introduces it. Older pods don't check whether a relayer's nonce was synced, and would give an imported relayer nonces from 0.
- **Grant the key.** In the AWS account that owns the key, append a statement to the key's key policy that lets the service's pod role use it. `aws kms put-key-policy` replaces the whole policy, so start from `aws kms get-key-policy` and add to it. In a key policy, `"Resource": "*"` is the key itself.

  ```json
  {
    "Sid": "AllowTxSitterMonolith",
    "Effect": "Allow",
    "Principal": { "AWS": "arn:aws:iam::<the service's account id>:role/<the service's pod role>" },
    "Action": ["kms:GetPublicKey", "kms:Sign"],
    "Resource": "*"
  }
  ```

- **Allow the key on the service's side.** A key of another account also needs the service's IAM policy to allow it: add the key ARN to the KMS statement of the service's role in the infrastructure repo (`crypto/<env>/us-east-1/tx-sitter-monolith.tf`).

tx-sitter-aws marks the queued transactions of a disabled relayer `Dropped`, and its broadcast Lambda and the cron of the network's other relayers work from snapshots that can still send them. So drain the relayer before disabling it:

1. **Pause the relayer's callers.**
2. **Drain tx-sitter-aws.** With the relayer still enabled there, wait until DynamoDB holds no `Queued` or `Pending` transactions of the relayer.
3. **Disable the relayer in tx-sitter-aws**, and wait for the Lambda timeout, so that invocations that started before are done.
4. **Check the chain.** On every RPC of the network in the config, `cast nonce --block pending <address>` equals `cast nonce --block latest <address>`.
5. **Add the relayer to the config** with a new id, `key = { kms = "<the key ARN>" }` and the key's `address`. Never reuse an id, also not when importing a key again. KMS looks up key ids in the service's own account, so use the full key ARN; aliases are refused, because they can be pointed at another key. The key must be in the service's region.
6. **Deploy, and confirm the first sync.** Startup fails, naming the relayer, while the key can't be read or derives another address than `address`. Then the worker logs `Synced the relayer's nonce from its chain for the first time` with the relayer id and `chain_nonce`, which should equal the address's transaction count. Until then sends to the relayer return 503. While the address has pending transactions that tx-sitter didn't send, the first sync is refused, logs `the key has pending transactions tx-sitter did not send` at error and is retried with every block: check steps 2 to 4 again.
7. **Check that the pod role can sign.** The startup check only reads the public key. Sign a test digest with the pod role's credentials, for example with `aws kms sign --key-id <the key ARN> --message-type DIGEST --signing-algorithm ECDSA_SHA_256 --message <a 32-byte digest, base64>`.
8. **Switch the callers** to the service with the relayer's API key, and resume them. The relayer's first transaction takes the chain's nonce.

To roll back, run the steps the other way around: pause the callers, wait with the relayer still enabled here until it has no unsent or pending transactions (`GET /1/api/<api key>/txs?unsent=true` and `?status=pending` return none), disable it in the config and deploy, check that `pending` equals `latest` on every RPC, then enable the relayer in tx-sitter-aws and switch the callers back. To import the key again later, add it with another new id.

### Migrating from the admin API

Earlier versions managed networks, relayers and API keys through `/1/admin/*`. Before deploying a version without it, export the database's state once:

```
tx-sitter --config <the service's config> config dump > static.toml
```

The command only reads the database. It prints the `[[networks]]` and `[[relayers]]` sections. RPC URLs, and local private keys, are printed as `${NAME}` references, and their values are listed on stderr: store those in the secrets manager, never in the config. The database holds one HTTP RPC and the WS RPC per network, so only those are exported; add fallback RPCs to `http_rpcs` by hand.

Deploy the dump next to the service's config, which the service loads first:

```
tx-sitter --config config.toml --config static.toml
```

Deploying the dumped config leaves the database as it is. Validation rejects relayers that share a chain id and address, which the admin API allowed: keep one of them in the config.

The admin API's basic auth was open while `server.username` and `server.password` were unset. Keep both set in the deployment until rolling back to an image with the admin API is no longer possible. This version ignores them.

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

To recover, reset the relayer with `tx-sitter relayer reset`, see "Operating guide". That deletes its unsent transactions, so their callers have to send them again. Sent ones stay pending.

### Relayer API v2

`POST /2/tx`, `GET /2/tx/{txId}` and `GET /2/events` take the relayer's API key in an `Authorization: Bearer <api key>` header, never in the URL. The v1 API under `/1/api/<api key>/` is unchanged.

A send may carry `metadata`, a JSON object of at most 64 KiB that tx-sitter stores and returns byte for byte. Sending a `txId` again with an identical request, metadata text included, returns the existing transaction without taking a nonce. A different request with the same `txId` returns 409. That holds while the transaction is kept: finalized transactions are deleted `service.tx_retention` (default `30d`) after they are mined.

Every v2 error is a JSON `{ "errorId": ..., "errorMessage": ... }` whose `errorId` stays stable. The OpenAPI schema lists them per status.

### Transaction events

`GET /2/events` returns the status changes of the relayer's transactions in order: `pending` when a transaction is broadcast, escalated or reorged out of its block, `mined` when it is in a block or reorged into another one, and `finalized`. Each event carries the transaction's id, hash and `metadata`. `wait=<seconds>` (at most 30) waits for an event when there is none.

A consumer keeps a checkpoint, the `nextCursor` of the last page it handled, and passes it back as `after`:

- Delivery is at least once. Store the new checkpoint only after every event of the page is handled, and make handling idempotent: a crash between the two returns the page again. A transaction id repeats legitimately, as when an escalation is followed by `mined`, or a reorg moves it back to `pending`.
- Keep one checkpoint per consumer and relayer. A cursor only works in the feed of the relayer whose API key got it; another relayer's key gets 400 `invalid_request`.
- 410 `cursor_expired` means the feed can't serve every event after the checkpoint, and the gap can't be recovered from the feed. Reconcile by reading the consumer's transactions with `GET /2/tx/{txId}`, then restart without `after`.

tx-sitter treats `metadata` as opaque. A consumer that replaces the legacy tx-sitter can wrap what it needs in an envelope of its own, such as `{ "transactionType": ..., "legacy": { ... } }`, and map the event's `chainId` to its network itself.

How the feed works:

- Events are stored in the database transaction that makes the change. The feed returns an event only once every database transaction that started writing before it has finished, so that no event is stored behind one already returned. A transaction that stays open holds the feed back; Postgres closes tx-sitter's own connections after 30s idle in a transaction, and the worker's `tx_events_held_back_seconds` gauge shows how long the feed has been held back.
- Events are deleted `service.event_retention` (default `7d`) after they are stored. A checkpoint from before deleted events gets 410. `service.event_retention` must not exceed `service.tx_retention`, so that a transaction, and its metadata, is kept as long as its events.
- Every process that serves the API listens for new events on one database connection, and waiting requests read the feed again every second in case a notification was lost. At most `server.max_event_waiters` (default `64`) requests wait at once per process; further ones get 503 `too_many_waiters` with a `Retry-After` of 1 to 3 seconds.
- `notification_queue_usage` is the fraction of Postgres' notification queue in use. Storing events, and with them every status change, fails once it is full.

Status changes made by a binary from before the event feed store no events. After rolling back to one and forward again, once no process of the older binary runs, make every checkpoint expire so that consumers reconcile:

```sql
INSERT INTO tx_events_pruned (tx_xid, id)
VALUES (pg_snapshot_xmin(pg_current_snapshot()), 0)
ON CONFLICT (singleton) DO UPDATE
SET tx_xid = EXCLUDED.tx_xid, id = EXCLUDED.id;
```

The feed and its order rely on Postgres transaction ids. A checkpoint ahead of the database's newest transaction gets 410, as after a restore to an earlier point. That misses checkpoints the restored database's transactions have already passed, so make every checkpoint expire after a restore too. Moving to a database with other transaction ids, such as a blue/green switchover through logical replication, needs the feed reset before tx-sitter starts on it, or status changes stop applying to existing transactions:

```sql
BEGIN;
UPDATE sent_transactions SET last_event_xid = NULL WHERE last_event_xid IS NOT NULL;
TRUNCATE tx_events;
INSERT INTO tx_events_pruned (tx_xid, id)
VALUES (pg_snapshot_xmin(pg_current_snapshot()), 0)
ON CONFLICT (singleton) DO UPDATE
SET tx_xid = EXCLUDED.tx_xid, id = EXCLUDED.id;
COMMIT;
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
