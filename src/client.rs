use crate::server::data::{
    CreateRelayerRequest, CreateRelayerResponse, SendTxRequest, SendTxResponse,
};
use crate::server::routes::network::NewNetworkInfo;

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

    pub async fn create_relayer(
        &self,
        req: &CreateRelayerRequest,
    ) -> eyre::Result<CreateRelayerResponse> {
        let response = self
            .client
            .post(&format!("{}/1/relayer/create", self.url))
            .json(req)
            .send()
            .await?;

        let response: CreateRelayerResponse = response.json().await?;

        Ok(response)
    }

    pub async fn send_tx(
        &self,
        req: &SendTxRequest,
    ) -> eyre::Result<SendTxResponse> {
        let response = self
            .client
            .post(&format!("{}/1/tx/send", self.url))
            .json(req)
            .send()
            .await?;

        Ok(response.json().await?)
    }

    pub async fn create_network(
        &self,
        chain_id: u64,
        req: &NewNetworkInfo,
    ) -> eyre::Result<()> {
        self.client
            .post(&format!("{}/1/network/{}", self.url, chain_id))
            .json(req)
            .send()
            .await?;

        // TODO: Handle status?

        Ok(())
    }
}
