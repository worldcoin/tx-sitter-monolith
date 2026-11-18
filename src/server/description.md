A transaction relayer service!

## Operating a relayer
Networks, relayers and API keys are defined in the service's config, see the README. A relayer accepts exactly the API keys whose hashes its config lists.

## Using an API key
The v2 endpoints take the API key as a bearer token, `Authorization: Bearer <api key>`. For example, `POST /2/tx` sends a transaction.

The v1 endpoints take it in the path instead, for example `POST /1/api/:api_token/tx`.
