# \RelayerV1Api

All URIs are relative to *http://localhost:8000*

Method | HTTP request | Description
------------- | ------------- | -------------
[**call_rpc**](RelayerV1Api.md#call_rpc) | **POST** /1/api/{api_token}/rpc | Relayer RPC
[**create_transaction**](RelayerV1Api.md#create_transaction) | **POST** /1/api/{api_token}/tx | Send Transaction
[**get_transaction**](RelayerV1Api.md#get_transaction) | **GET** /1/api/{api_token}/tx/{tx_id} | Get Transaction
[**get_transactions**](RelayerV1Api.md#get_transactions) | **GET** /1/api/{api_token}/txs | Get Transactions



## call_rpc

> serde_json::Value call_rpc(api_token, rpc_request)
Relayer RPC

### Parameters


Name | Type | Description  | Required | Notes
------------- | ------------- | ------------- | ------------- | -------------
**api_token** | **String** |  | [required] |
**rpc_request** | [**RpcRequest**](RpcRequest.md) |  | [required] |

### Return type

[**serde_json::Value**](serde_json::Value.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: application/json; charset=utf-8
- **Accept**: application/json; charset=utf-8

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)


## create_transaction

> models::SendTxResponse create_transaction(api_token, send_tx_request)
Send Transaction

### Parameters


Name | Type | Description  | Required | Notes
------------- | ------------- | ------------- | ------------- | -------------
**api_token** | **String** |  | [required] |
**send_tx_request** | [**SendTxRequest**](SendTxRequest.md) |  | [required] |

### Return type

[**models::SendTxResponse**](SendTxResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: application/json; charset=utf-8
- **Accept**: application/json; charset=utf-8

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)


## get_transaction

> models::GetTxResponse get_transaction(api_token, tx_id)
Get Transaction

### Parameters


Name | Type | Description  | Required | Notes
------------- | ------------- | ------------- | ------------- | -------------
**api_token** | **String** |  | [required] |
**tx_id** | **String** |  | [required] |

### Return type

[**models::GetTxResponse**](GetTxResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json; charset=utf-8

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)


## get_transactions

> Vec<models::GetTxResponse> get_transactions(api_token, status, unsent)
Get Transactions

### Parameters


Name | Type | Description  | Required | Notes
------------- | ------------- | ------------- | ------------- | -------------
**api_token** | **String** |  | [required] |
**status** | Option<[**TxStatus**](.md)> | Optional tx status to filter by |  |
**unsent** | Option<**bool**> | Fetch unsent txs, overrides the status query |  |[default to false]

### Return type

[**Vec<models::GetTxResponse>**](GetTxResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json; charset=utf-8

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)

