# TxEvent

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**tx_id** | **String** |  | 
**relayer_id** | **String** |  | 
**chain_id** | **i32** |  | 
**status** | [**models::TxStatus**](TxStatus.md) |  | 
**tx_hash** | Option<[**base_api_types::H256**](base_api_types::H256.md)> | Hash of the broadcast transaction that is mined, or else of the latest one broadcast | [optional][default to 0x0000000000000000000000000000000000000000000000000000000000000000]
**metadata** | Option<[**std::collections::HashMap<String, serde_json::Value>**](serde_json::Value.md)> | `null` for transactions sent through the v1 API, and once the transaction is deleted `service.tx_retention` after it is mined | [optional]
**created_at** | **String** | When tx-sitter stored the change | 

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


