use reqwest::Response;
use thiserror::Error;

use crate::api_key::ApiKey;
use crate::types::{
    CreateApiKeyResponse, CreateRelayerRequest, CreateRelayerResponse,
    GetTxResponse, NewNetworkInfo, RelayerUpdate, SendTxRequest,
    SendTxResponse,
};

pub struct TxSitterClient {
    client: reqwest::Client,
    url: String,

    credentials: Option<(String, String)>,
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("Reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),

    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("API error: {0}")]
    TxSitter(reqwest::StatusCode, String),

    #[error("Invalid API key: {0}")]
    InvalidApiKey(eyre::Error),
}

impl TxSitterClient {
    pub fn new(url: impl ToString) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: url.to_string(),
            credentials: None,
        }
    }

    pub fn with_credentials(
        mut self,
        username: impl ToString,
        password: impl ToString,
    ) -> Self {
        self.credentials = Some((username.to_string(), password.to_string()));
        self
    }

    fn creds(&self) -> (&str, &str) {
        self.credentials
            .as_ref()
            .map(|(u, p)| (u.as_str(), p.as_str()))
            .unwrap_or_default()
    }

    async fn post<R>(&self, url: &str) -> Result<R, ClientError>
    where
        R: serde::de::DeserializeOwned,
    {
        let (username, password) = self.creds();

        let response = self
            .client
            .post(url)
            .basic_auth(username, Some(password))
            .send()
            .await?;

        let response = Self::validate_response(response).await?;

        Ok(response.json().await?)
    }

    async fn json_post<T, R>(
        &self,
        url: &str,
        body: T,
    ) -> Result<R, ClientError>
    where
        T: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
        let (username, password) = self.creds();

        let response = self
            .client
            .post(url)
            .json(&body)
            .basic_auth(username, Some(password))
            .send()
            .await?;

        let response = Self::validate_response(response).await?;

        Ok(response.json().await?)
    }

    async fn json_get<R>(&self, url: &str) -> Result<R, ClientError>
    where
        R: serde::de::DeserializeOwned,
    {
        let (username, password) = self.creds();

        let response = self
            .client
            .get(url)
            .basic_auth(username, Some(password))
            .send()
            .await?;

        let response = Self::validate_response(response).await?;

        Ok(response.json().await?)
    }

    async fn validate_response(
        response: Response,
    ) -> Result<Response, ClientError> {
        let status = response.status();
        if !status.is_success() {
            let body: String = response.text().await?;
            return Err(ClientError::TxSitter(status, body));
        }

        Ok(response)
    }

    pub async fn create_relayer(
        &self,
        req: &CreateRelayerRequest,
    ) -> Result<CreateRelayerResponse, ClientError> {
        self.json_post(&format!("{}/1/admin/relayer", self.url), req)
            .await
    }

    pub async fn create_relayer_api_key(
        &self,
        relayer_id: &str,
    ) -> Result<CreateApiKeyResponse, ClientError> {
        self.post(&format!("{}/1/admin/relayer/{relayer_id}/key", self.url,))
            .await
    }

    pub async fn update_relayer(
        &self,
        relayer_id: &str,
        relayer_update: RelayerUpdate,
    ) -> Result<(), ClientError> {
        let url: &str = &format!("{}/1/admin/relayer/{relayer_id}", self.url);

        let (username, password) = self.creds();

        let response = self
            .client
            .post(url)
            .json(&relayer_update)
            .basic_auth(username, Some(password))
            .send()
            .await?;

        let _response = Self::validate_response(response).await?;

        Ok(())
    }

    pub async fn send_tx(
        &self,
        api_key: &ApiKey,
        req: &SendTxRequest,
    ) -> Result<SendTxResponse, ClientError> {
        self.json_post(
            &format!(
                "{}/1/api/{}/tx",
                self.url,
                api_key.reveal().map_err(ClientError::InvalidApiKey)?
            ),
            req,
        )
        .await
    }

    pub async fn get_tx(
        &self,
        api_key: &ApiKey,
        tx_id: &str,
    ) -> Result<GetTxResponse, ClientError> {
        self.json_get(&format!(
            "{}/1/api/{}/tx/{tx_id}",
            self.url,
            api_key.reveal().map_err(ClientError::InvalidApiKey)?,
            tx_id = tx_id
        ))
        .await
    }

    pub async fn create_network(
        &self,
        chain_id: u64,
        req: &NewNetworkInfo,
    ) -> Result<(), ClientError> {
        let response = self
            .client
            .post(&format!("{}/1/admin/network/{}", self.url, chain_id))
            .json(&req)
            .send()
            .await?;

        Self::validate_response(response).await?;

        Ok(())
    }
}

impl ClientError {
    pub fn tx_sitter_message(&self) -> Option<&str> {
        match self {
            Self::TxSitter(_, s) => Some(s),
            _ => None,
        }
    }
}
