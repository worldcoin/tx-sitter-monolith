use reqwest::Response;

use crate::api_key::ApiKey;
use crate::server::routes::network::NewNetworkInfo;
use crate::server::routes::relayer::{
    CreateApiKeyResponse, CreateRelayerRequest, CreateRelayerResponse,
};
use crate::server::routes::transaction::{
    GetTxResponse, SendTxRequest, SendTxResponse,
};

pub struct TxSitterClient {
    client: reqwest::Client,
    url: String,
}

impl TxSitterClient {
    pub fn new(url: impl ToString) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: url.to_string(),
        }
    }

    async fn post<R>(&self, url: &str) -> eyre::Result<R>
    where
        R: serde::de::DeserializeOwned,
    {
        let response = self.client.post(url).send().await?;

        let response = Self::validate_response(response).await?;

        Ok(response.json().await?)
    }

    async fn json_post<T, R>(&self, url: &str, body: T) -> eyre::Result<R>
    where
        T: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
        let response = self.client.post(url).json(&body).send().await?;

        let response = Self::validate_response(response).await?;

        Ok(response.json().await?)
    }

    async fn json_get<R>(&self, url: &str) -> eyre::Result<R>
    where
        R: serde::de::DeserializeOwned,
    {
        let response = self.client.get(url).send().await?;

        let response = Self::validate_response(response).await?;

        Ok(response.json().await?)
    }

    async fn validate_response(response: Response) -> eyre::Result<Response> {
        if !response.status().is_success() {
            let body = response.text().await?;

            return Err(eyre::eyre!("{body}"));
        }

        Ok(response)
    }
    pub async fn create_relayer(
        &self,
        req: &CreateRelayerRequest,
    ) -> eyre::Result<CreateRelayerResponse> {
        self.json_post(&format!("{}/1/admin/relayer", self.url), req)
            .await
    }

    pub async fn create_relayer_api_key(
        &self,
        relayer_id: &str,
    ) -> eyre::Result<CreateApiKeyResponse> {
        self.post(&format!("{}/1/admin/relayer/{relayer_id}/key", self.url,))
            .await
    }

    pub async fn send_tx(
        &self,
        api_key: &ApiKey,
        req: &SendTxRequest,
    ) -> eyre::Result<SendTxResponse> {
        self.json_post(&format!("{}/1/api/{api_key}/tx", self.url), req)
            .await
    }

    pub async fn get_tx(
        &self,
        api_key: &ApiKey,
        tx_id: &str,
    ) -> eyre::Result<GetTxResponse> {
        Ok(self
            .json_get(&format!(
                "{}/1/api/{api_key}/tx/{tx_id}",
                self.url,
                api_key = api_key,
                tx_id = tx_id
            ))
            .await?)
    }

    pub async fn create_network(
        &self,
        chain_id: u64,
        req: &NewNetworkInfo,
    ) -> eyre::Result<()> {
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
