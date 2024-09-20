# GetTxResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**tx_id** | **String** |  | 
**to** | [**base_api_types::Address**](base_api_types::Address.md) | Hex encoded ethereum address | 
**data** | Option<[**base_api_types::HexBytes**](base_api_types::HexBytes.md)> |  | [optional]
**value** | [**base_api_types::DecimalU256**](base_api_types::DecimalU256.md) | A decimal 256-bit unsigned integer | [default to 0]
**gas_limit** | [**base_api_types::DecimalU256**](base_api_types::DecimalU256.md) | A decimal 256-bit unsigned integer | [default to 0]
**nonce** | **i32** |  | 
**tx_hash** | Option<[**base_api_types::H256**](base_api_types::H256.md)> | A hex encoded 256-bit hash | [optional][default to 0x0000000000000000000000000000000000000000000000000000000000000000]
**status** | Option<[**models::TxStatus**](TxStatus.md)> |  | [optional]

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


