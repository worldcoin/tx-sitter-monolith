/*
 * Tx Sitter
 *
 * A transaction relayer service!  ## Operating a relayer Below is a guide on using this service. Note that steps 1 through 4 require authentication using HTTP Basic auth. Using swagger explorer make sure to click the authorize button and use the correct credentials. Default dev creds are `admin:admin`.  ### 1. Setup a network tx-sitter keeps track of supported networks in its internal database. In order to be able to create any relayers at least one network must be present. To add a network use the `POST /1/admin/networks/:chain_id` endpoint.  To see the list of currently added networks use the `GET /1/admin/networks` endpoint.  ### 2. Create a relayer A relayer is an abstraction layer on top of a private key stored locally (for testing purposes only!) or using a secrets manager (currently only AWS KMS is supported).  To create a relayer use the `POST /1/admin/relayer` endpoint. The data returned will contain a relayer id, make sure to copy it to the clipboard.  ### 3. Create an API key By itself a relayer is not very useful. In order to send transactions one must create an API key. To do that use the `POST /1/admin/relayer/:relayer_id/key` endpoint. **Make sure to copy the API key from the response. It's not possible to recover it!** But it's always possible to create a new one.  ### 4. Use the API key Once an API keys has been created it's possible to use the relayer api to, among other things, send transactions.  You can use the `POST /1/api/:api_token/tx` endpoint to create a transaction.
 *
 * The version of the OpenAPI document: 0.1.0
 *
 * Generated by: https://openapi-generator.tech
 */

use reqwest;
use serde::{Deserialize, Serialize};

use super::{configuration, Error};
use crate::apis::ResponseContent;
use crate::models;

/// struct for passing parameters to the method [`call_rpc`]
#[derive(Clone, Debug)]
pub struct CallRpcParams {
    pub api_token: String,
    pub rpc_request: models::RpcRequest,
}

/// struct for passing parameters to the method [`create_transaction`]
#[derive(Clone, Debug)]
pub struct CreateTransactionParams {
    pub api_token: String,
    pub send_tx_request: models::SendTxRequest,
}

/// struct for passing parameters to the method [`get_transaction`]
#[derive(Clone, Debug)]
pub struct GetTransactionParams {
    pub api_token: String,
    pub tx_id: String,
}

/// struct for passing parameters to the method [`get_transactions`]
#[derive(Clone, Debug)]
pub struct GetTransactionsParams {
    pub api_token: String,
    /// Optional tx status to filter by
    pub status: Option<models::TxStatus>,
    /// Fetch unsent txs, overrides the status query
    pub unsent: Option<bool>,
}

/// struct for typed errors of method [`call_rpc`]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CallRpcError {
    UnknownValue(serde_json::Value),
}

/// struct for typed errors of method [`create_transaction`]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CreateTransactionError {
    UnknownValue(serde_json::Value),
}

/// struct for typed errors of method [`get_transaction`]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GetTransactionError {
    UnknownValue(serde_json::Value),
}

/// struct for typed errors of method [`get_transactions`]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GetTransactionsError {
    UnknownValue(serde_json::Value),
}

pub async fn call_rpc(
    configuration: &configuration::Configuration,
    params: CallRpcParams,
) -> Result<serde_json::Value, Error<CallRpcError>> {
    let local_var_configuration = configuration;

    // unbox the parameters
    let api_token = params.api_token;
    let rpc_request = params.rpc_request;

    let local_var_client = &local_var_configuration.client;

    let local_var_uri_str = format!(
        "{}/1/api/{api_token}/rpc",
        local_var_configuration.base_path,
        api_token = crate::apis::urlencode(api_token)
    );
    let mut local_var_req_builder = local_var_client
        .request(reqwest::Method::POST, local_var_uri_str.as_str());

    if let Some(ref local_var_user_agent) = local_var_configuration.user_agent {
        local_var_req_builder = local_var_req_builder
            .header(reqwest::header::USER_AGENT, local_var_user_agent.clone());
    }
    local_var_req_builder = local_var_req_builder.json(&rpc_request);

    let local_var_req = local_var_req_builder.build()?;
    let local_var_resp = local_var_client.execute(local_var_req).await?;

    let local_var_status = local_var_resp.status();
    let local_var_content = local_var_resp.text().await?;

    if !local_var_status.is_client_error()
        && !local_var_status.is_server_error()
    {
        serde_json::from_str(&local_var_content).map_err(Error::from)
    } else {
        let local_var_entity: Option<CallRpcError> =
            serde_json::from_str(&local_var_content).ok();
        let local_var_error = ResponseContent {
            status: local_var_status,
            content: local_var_content,
            entity: local_var_entity,
        };
        Err(Error::ResponseError(local_var_error))
    }
}

pub async fn create_transaction(
    configuration: &configuration::Configuration,
    params: CreateTransactionParams,
) -> Result<models::SendTxResponse, Error<CreateTransactionError>> {
    let local_var_configuration = configuration;

    // unbox the parameters
    let api_token = params.api_token;
    let send_tx_request = params.send_tx_request;

    let local_var_client = &local_var_configuration.client;

    let local_var_uri_str = format!(
        "{}/1/api/{api_token}/tx",
        local_var_configuration.base_path,
        api_token = crate::apis::urlencode(api_token)
    );
    let mut local_var_req_builder = local_var_client
        .request(reqwest::Method::POST, local_var_uri_str.as_str());

    if let Some(ref local_var_user_agent) = local_var_configuration.user_agent {
        local_var_req_builder = local_var_req_builder
            .header(reqwest::header::USER_AGENT, local_var_user_agent.clone());
    }
    local_var_req_builder = local_var_req_builder.json(&send_tx_request);

    let local_var_req = local_var_req_builder.build()?;
    let local_var_resp = local_var_client.execute(local_var_req).await?;

    let local_var_status = local_var_resp.status();
    let local_var_content = local_var_resp.text().await?;

    if !local_var_status.is_client_error()
        && !local_var_status.is_server_error()
    {
        serde_json::from_str(&local_var_content).map_err(Error::from)
    } else {
        let local_var_entity: Option<CreateTransactionError> =
            serde_json::from_str(&local_var_content).ok();
        let local_var_error = ResponseContent {
            status: local_var_status,
            content: local_var_content,
            entity: local_var_entity,
        };
        Err(Error::ResponseError(local_var_error))
    }
}

pub async fn get_transaction(
    configuration: &configuration::Configuration,
    params: GetTransactionParams,
) -> Result<models::GetTxResponse, Error<GetTransactionError>> {
    let local_var_configuration = configuration;

    // unbox the parameters
    let api_token = params.api_token;
    let tx_id = params.tx_id;

    let local_var_client = &local_var_configuration.client;

    let local_var_uri_str = format!(
        "{}/1/api/{api_token}/tx/{tx_id}",
        local_var_configuration.base_path,
        api_token = crate::apis::urlencode(api_token),
        tx_id = crate::apis::urlencode(tx_id)
    );
    let mut local_var_req_builder = local_var_client
        .request(reqwest::Method::GET, local_var_uri_str.as_str());

    if let Some(ref local_var_user_agent) = local_var_configuration.user_agent {
        local_var_req_builder = local_var_req_builder
            .header(reqwest::header::USER_AGENT, local_var_user_agent.clone());
    }

    let local_var_req = local_var_req_builder.build()?;
    let local_var_resp = local_var_client.execute(local_var_req).await?;

    let local_var_status = local_var_resp.status();
    let local_var_content = local_var_resp.text().await?;

    if !local_var_status.is_client_error()
        && !local_var_status.is_server_error()
    {
        serde_json::from_str(&local_var_content).map_err(Error::from)
    } else {
        let local_var_entity: Option<GetTransactionError> =
            serde_json::from_str(&local_var_content).ok();
        let local_var_error = ResponseContent {
            status: local_var_status,
            content: local_var_content,
            entity: local_var_entity,
        };
        Err(Error::ResponseError(local_var_error))
    }
}

pub async fn get_transactions(
    configuration: &configuration::Configuration,
    params: GetTransactionsParams,
) -> Result<Vec<models::GetTxResponse>, Error<GetTransactionsError>> {
    let local_var_configuration = configuration;

    // unbox the parameters
    let api_token = params.api_token;
    let status = params.status;
    let unsent = params.unsent;

    let local_var_client = &local_var_configuration.client;

    let local_var_uri_str = format!(
        "{}/1/api/{api_token}/txs",
        local_var_configuration.base_path,
        api_token = crate::apis::urlencode(api_token)
    );
    let mut local_var_req_builder = local_var_client
        .request(reqwest::Method::GET, local_var_uri_str.as_str());

    if let Some(ref local_var_str) = status {
        local_var_req_builder = local_var_req_builder
            .query(&[("status", &local_var_str.to_string())]);
    }
    if let Some(ref local_var_str) = unsent {
        local_var_req_builder = local_var_req_builder
            .query(&[("unsent", &local_var_str.to_string())]);
    }
    if let Some(ref local_var_user_agent) = local_var_configuration.user_agent {
        local_var_req_builder = local_var_req_builder
            .header(reqwest::header::USER_AGENT, local_var_user_agent.clone());
    }

    let local_var_req = local_var_req_builder.build()?;
    let local_var_resp = local_var_client.execute(local_var_req).await?;

    let local_var_status = local_var_resp.status();
    let local_var_content = local_var_resp.text().await?;

    if !local_var_status.is_client_error()
        && !local_var_status.is_server_error()
    {
        serde_json::from_str(&local_var_content).map_err(Error::from)
    } else {
        let local_var_entity: Option<GetTransactionsError> =
            serde_json::from_str(&local_var_content).ok();
        let local_var_error = ResponseContent {
            status: local_var_status,
            content: local_var_content,
            entity: local_var_entity,
        };
        Err(Error::ResponseError(local_var_error))
    }
}
