# \AdminV1Api

All URIs are relative to *http://localhost:8000*

Method | HTTP request | Description
------------- | ------------- | -------------
[**create_network**](AdminV1Api.md#create_network) | **POST** /1/admin/network/{chain_id} | Create Network
[**create_relayer**](AdminV1Api.md#create_relayer) | **POST** /1/admin/relayer | Create Relayer
[**get_networks**](AdminV1Api.md#get_networks) | **GET** /1/admin/networks | Get Networks
[**get_relayer**](AdminV1Api.md#get_relayer) | **GET** /1/admin/relayer/{relayer_id} | Get Relayer
[**get_relayers**](AdminV1Api.md#get_relayers) | **GET** /1/admin/relayers | Get Relayers
[**relayer_create_api_key**](AdminV1Api.md#relayer_create_api_key) | **POST** /1/admin/relayer/{relayer_id}/key | Create Relayer API Key
[**reset_relayer**](AdminV1Api.md#reset_relayer) | **POST** /1/admin/relayer/{relayer_id}/reset | Reset Relayer transactions
[**update_relayer**](AdminV1Api.md#update_relayer) | **POST** /1/admin/relayer/{relayer_id} | Update Relayer



## create_network

> create_network(chain_id, create_network_request)
Create Network

### Parameters


Name | Type | Description  | Required | Notes
------------- | ------------- | ------------- | ------------- | -------------
**chain_id** | **i32** |  | [required] |
**create_network_request** | [**CreateNetworkRequest**](CreateNetworkRequest.md) |  | [required] |

### Return type

 (empty response body)

### Authorization

[BasicAuth](../README.md#BasicAuth)

### HTTP request headers

- **Content-Type**: application/json; charset=utf-8
- **Accept**: Not defined

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)


## create_relayer

> models::CreateRelayerResponse create_relayer(create_relayer_request)
Create Relayer

### Parameters


Name | Type | Description  | Required | Notes
------------- | ------------- | ------------- | ------------- | -------------
**create_relayer_request** | [**CreateRelayerRequest**](CreateRelayerRequest.md) |  | [required] |

### Return type

[**models::CreateRelayerResponse**](CreateRelayerResponse.md)

### Authorization

[BasicAuth](../README.md#BasicAuth)

### HTTP request headers

- **Content-Type**: application/json; charset=utf-8
- **Accept**: application/json; charset=utf-8

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)


## get_networks

> Vec<models::NetworkResponse> get_networks()
Get Networks

### Parameters

This endpoint does not need any parameter.

### Return type

[**Vec<models::NetworkResponse>**](NetworkResponse.md)

### Authorization

[BasicAuth](../README.md#BasicAuth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json; charset=utf-8

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)


## get_relayer

> models::RelayerResponse get_relayer(relayer_id)
Get Relayer

### Parameters


Name | Type | Description  | Required | Notes
------------- | ------------- | ------------- | ------------- | -------------
**relayer_id** | **String** |  | [required] |

### Return type

[**models::RelayerResponse**](RelayerResponse.md)

### Authorization

[BasicAuth](../README.md#BasicAuth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json; charset=utf-8

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)


## get_relayers

> Vec<models::RelayerResponse> get_relayers()
Get Relayers

### Parameters

This endpoint does not need any parameter.

### Return type

[**Vec<models::RelayerResponse>**](RelayerResponse.md)

### Authorization

[BasicAuth](../README.md#BasicAuth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json; charset=utf-8

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)


## relayer_create_api_key

> models::CreateApiKeyResponse relayer_create_api_key(relayer_id)
Create Relayer API Key

### Parameters


Name | Type | Description  | Required | Notes
------------- | ------------- | ------------- | ------------- | -------------
**relayer_id** | **String** |  | [required] |

### Return type

[**models::CreateApiKeyResponse**](CreateApiKeyResponse.md)

### Authorization

[BasicAuth](../README.md#BasicAuth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json; charset=utf-8

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)


## reset_relayer

> reset_relayer(relayer_id)
Reset Relayer transactions

Purges unsent transactions, useful for unstucking the relayer

### Parameters


Name | Type | Description  | Required | Notes
------------- | ------------- | ------------- | ------------- | -------------
**relayer_id** | **String** |  | [required] |

### Return type

 (empty response body)

### Authorization

[BasicAuth](../README.md#BasicAuth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: Not defined

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)


## update_relayer

> update_relayer(relayer_id, relayer_update_request)
Update Relayer

### Parameters


Name | Type | Description  | Required | Notes
------------- | ------------- | ------------- | ------------- | -------------
**relayer_id** | **String** |  | [required] |
**relayer_update_request** | [**RelayerUpdateRequest**](RelayerUpdateRequest.md) |  | [required] |

### Return type

 (empty response body)

### Authorization

[BasicAuth](../README.md#BasicAuth)

### HTTP request headers

- **Content-Type**: application/json; charset=utf-8
- **Accept**: Not defined

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)

