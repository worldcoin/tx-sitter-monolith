# GetTxResponseV2

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**tx_id** | **String** |  | 
**relayer_id** | **String** |  | 
**chain_id** | **i32** |  | 
**to** | [**base_api_types::Address**](base_api_types::Address.md) | Hex encoded ethereum address | 
**data** | [**base_api_types::HexBytes**](base_api_types::HexBytes.md) |  | 
**value** | [**base_api_types::DecimalU256**](base_api_types::DecimalU256.md) | A decimal 256-bit unsigned integer | [default to 0]
**gas_limit** | [**base_api_types::DecimalU256**](base_api_types::DecimalU256.md) | A decimal 256-bit unsigned integer | [default to 0]
**nonce** | **i32** |  | 
**priority** | [**models::TransactionPriority**](TransactionPriority.md) |  | 
**metadata** | Option<[**std::collections::HashMap<String, serde_json::Value>**](serde_json::Value.md)> | `null` for transactions sent through the v1 API | [optional]
**created_at** | **String** | When tx-sitter stored the transaction | 
**status** | [**models::TxStatusV2**](TxStatusV2.md) |  | 
**tx_hash** | Option<[**base_api_types::H256**](base_api_types::H256.md)> | Hash of the broadcast transaction that is mined, or else of the latest one broadcast | [optional][default to 0x0000000000000000000000000000000000000000000000000000000000000000]
**mined_at** | Option<**String**> | Timestamp of the block the transaction was mined in | [optional]

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


