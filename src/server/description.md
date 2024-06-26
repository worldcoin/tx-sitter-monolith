A transaction relayer service!

## Operating a relayer
Below is a guide on using this service. Note that septs 1 through 4 require authentication using HTTP Basic auth. Using swagger explorer make sure to click the authorize button and use the correct credentials. Default dev creds are `admin:admin`.

### 1. Setup a network
tx-sitter keeps track of supported networks in its internal database. In order to be able to create any relayers at least one network must be present. To add a network use the `POST /1/admin/networks/:chain_id` endpoint.

To see the list of currently added networks use the `GET /1/admin/networks` endpoint.

### 2. Create a relayer
A relayer is an abstraction layer on top of a private key stored locally (for testing purposes only!) or using a secrets manager (currently only AWS KMS is supported).

To create a relayer use the `POST /1/admin/relayer` endpoint. The data returned will contain a relayer id, make sure to copy it to the clipboard.

### 3. Create an API key
By itself a relayer is not very useful. In order to send transactions one must create an API key. To do that use the `POST /1/admin/relayer/:relayer_id/key` endpoint. **Make sure to copy the API key from the response. It's not possible to recover it!** But it's always possible to create a new one.

### 4. Use the API key
Once an API keys has been created it's possible to use the relayer api to, among other things, send transactions.

You can use the `POST /1/api/:api_token/tx` endpoint to create a transaction.
