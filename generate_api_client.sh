#!/usr/bin/env bash

set -eux

cargo run --bin api_spec_generator > ./schema.yaml

docker run --rm -v "${PWD}:/local" --user "$(id -u):$(id -g)" -- openapitools/openapi-generator-cli generate \
  -i /local/schema.yaml \
  -g rust \
  -o /local/crates/tx-sitter-client \
  -t /local/client-template \
  --skip-validate-spec \
  --additional-properties=packageName=tx-sitter-client,supportMiddleware=true,useSingleRequestParameter=true,avoidBoxedModels=true \
  --type-mappings=\
address=base_api_types::Address,\
decimal-u256=base_api_types::DecimalU256,\
h256=base_api_types::H256,\
bytes=base_api_types::HexBytes,\
hex-u256=base_api_types::HexU256
