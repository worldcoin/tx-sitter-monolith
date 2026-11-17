# SendTxRequestV2

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**to** | [**base_api_types::Address**](base_api_types::Address.md) | Hex encoded ethereum address | 
**value** | [**base_api_types::DecimalU256**](base_api_types::DecimalU256.md) | Transaction value | [default to 0]
**data** | Option<[**base_api_types::HexBytes**](base_api_types::HexBytes.md)> |  | [optional]
**gas_limit** | [**base_api_types::DecimalU256**](base_api_types::DecimalU256.md) | Transaction gas limit | [default to 0]
**priority** | Option<[**models::TransactionPriority**](TransactionPriority.md)> |  | [optional]
**tx_id** | Option<**String**> | An optional transaction id. If not provided tx-sitter will generate a UUID.  Sending an id again with an identical request returns the existing transaction and changes nothing. Requests are identical when their fields are equal and their `metadata` is the same JSON text, byte for byte, as a retry that re-sends the same body is. Sending the id with a different request, or reusing an id sent through the v1 API, returns 409.  An id stays taken while its transaction is kept: tx-sitter deletes finalized transactions `service.tx_retention` (30 days by default) after they are mined. After that a repeat creates a new transaction. | [optional]
**metadata** | Option<[**std::collections::HashMap<String, serde_json::Value>**](serde_json::Value.md)> | A JSON object stored with the transaction and returned exactly as sent, at most 64 KiB of JSON text. | [optional]

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


