# GetTxEventsResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**events** | [**Vec<models::TxEvent>**](TxEvent.md) | The relayer's events after `after`, in the order they happened | 
**next_cursor** | Option<**String**> | Pass as `after` to get the events after these. It is `after` when there are none, and `null` when there are none and `after` was left out. | [optional]

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


