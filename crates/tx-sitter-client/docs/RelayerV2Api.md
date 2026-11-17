# \RelayerV2Api

All URIs are relative to *http://localhost:8000*

Method | HTTP request | Description
------------- | ------------- | -------------
[**create_transaction_v2**](RelayerV2Api.md#create_transaction_v2) | **POST** /2/tx | Send Transaction
[**get_transaction_v2**](RelayerV2Api.md#get_transaction_v2) | **GET** /2/tx/{tx_id} | Get Transaction



## create_transaction_v2

> models::SendTxResponse create_transaction_v2(send_tx_request_v2)
Send Transaction

Sending a `txId` again with an identical request returns the same `txId` and changes nothing, even while the relayer is disabled or its queue is full. Sending it with a different request returns 409.

### Parameters


Name | Type | Description  | Required | Notes
------------- | ------------- | ------------- | ------------- | -------------
**send_tx_request_v2** | [**SendTxRequestV2**](SendTxRequestV2.md) |  | [required] |

### Return type

[**models::SendTxResponse**](SendTxResponse.md)

### Authorization

[BearerApiKey](../README.md#BearerApiKey)

### HTTP request headers

- **Content-Type**: application/json; charset=utf-8
- **Accept**: application/json; charset=utf-8

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)


## get_transaction_v2

> models::GetTxResponseV2 get_transaction_v2(tx_id)
Get Transaction

Returns 404 for a transaction of another relayer.

### Parameters


Name | Type | Description  | Required | Notes
------------- | ------------- | ------------- | ------------- | -------------
**tx_id** | **String** |  | [required] |

### Return type

[**models::GetTxResponseV2**](GetTxResponseV2.md)

### Authorization

[BearerApiKey](../README.md#BearerApiKey)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json; charset=utf-8

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)

