# SendTxRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**to** | [**base_api_types::Address**](base_api_types::Address.md) | Hex encoded ethereum address | 
**value** | [**base_api_types::DecimalU256**](base_api_types::DecimalU256.md) | Transaction value | [default to 0]
**data** | Option<[**base_api_types::HexBytes**](base_api_types::HexBytes.md)> |  | [optional]
**gas_limit** | [**base_api_types::DecimalU256**](base_api_types::DecimalU256.md) | Transaction gas limit | [default to 0]
**priority** | Option<[**models::TransactionPriority**](TransactionPriority.md)> |  | [optional]
**tx_id** | Option<**String**> | An optional transaction id. If not provided tx-sitter will generate a UUID.  Can be used to provide idempotency for the transaction. | [optional]
**blobs** | Option<[**Vec<Vec<i32>>**](Vec.md)> |  | [optional]

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


