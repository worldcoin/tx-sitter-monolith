/*
 * Tx Sitter
 *
 * A transaction relayer service!  ## Operating a relayer Below is a guide on using this service. Note that septs 1 through 4 require authentication using HTTP Basic auth. Using swagger explorer make sure to click the authorize button and use the correct credentials. Default dev creds are `admin:admin`.  ### 1. Setup a network tx-sitter keeps track of supported networks in its internal database. In order to be able to create any relayers at least one network must be present. To add a network use the `POST /1/admin/networks/:chain_id` endpoint.  To see the list of currently added networks use the `GET /1/admin/networks` endpoint.  ### 2. Create a relayer A relayer is an abstraction layer on top of a private key stored locally (for testing purposes only!) or using a secrets manager (currently only AWS KMS is supported).  To create a relayer use the `POST /1/admin/relayer` endpoint. The data returned will contain a relayer id, make sure to copy it to the clipboard.  ### 3. Create an API key By itself a relayer is not very useful. In order to send transactions one must create an API key. To do that use the `POST /1/admin/relayer/:relayer_id/key` endpoint. **Make sure to copy the API key from the response. It's not possible to recover it!** But it's always possible to create a new one.  ### 4. Use the API key Once an API keys has been created it's possible to use the relayer api to, among other things, send transactions.  You can use the `POST /1/api/:api_token/tx` endpoint to create a transaction.
 *
 * The version of the OpenAPI document: 0.1.0
 *
 * Generated by: https://openapi-generator.tech
 */

#[derive(Debug, Clone)]
pub struct Configuration {
    pub base_path: String,
    pub user_agent: Option<String>,
    pub client: reqwest_middleware::ClientWithMiddleware,
    pub basic_auth: Option<BasicAuth>,
    pub oauth_access_token: Option<String>,
    pub bearer_access_token: Option<String>,
    pub api_key: Option<ApiKey>,
    // TODO: take an oauth2 token source, similar to the go one
}

pub type BasicAuth = (String, Option<String>);

#[derive(Debug, Clone)]
pub struct ApiKey {
    pub prefix: Option<String>,
    pub key: String,
}

impl Default for Configuration {
    fn default() -> Self {
        Configuration {
            base_path: "http://localhost:3000".to_owned(),
            user_agent: Some("OpenAPI-Generator/0.1.0/rust".to_owned()),
            client: reqwest_middleware::ClientBuilder::new(
                reqwest::Client::new(),
            )
            .build(),
            basic_auth: None,
            oauth_access_token: None,
            bearer_access_token: None,
            api_key: None,
        }
    }
}

pub struct ConfigurationBuilder {
    pub base_path: Option<String>,
    pub user_agent: Option<String>,
    pub basic_auth: Option<BasicAuth>,
}

impl ConfigurationBuilder {
    pub fn new() -> ConfigurationBuilder {
        ConfigurationBuilder {
            base_path: None,
            user_agent: None,
            basic_auth: None,
        }
    }

    pub fn base_path(mut self, base_path: String) -> Self {
        self.base_path = Some(base_path);
        self
    }

    pub fn user_agent(mut self, user_agent: String) -> Self {
        self.user_agent = Some(user_agent);
        self
    }

    pub fn basic_auth(mut self, user: String, pass: Option<String>) -> Self {
        self.basic_auth = Some((user, pass));
        self
    }

    pub fn build(self) -> Configuration {
        let mut conf: Configuration = Default::default();

        if let Some(base_path) = self.base_path {
            conf.base_path = base_path;
        }

        if let Some(user_agent) = self.user_agent {
            conf.user_agent = Some(user_agent);
        }

        if let Some(basic_auth) = self.basic_auth {
            conf.basic_auth = Some(basic_auth);
        }

        conf
    }
}

impl Default for ConfigurationBuilder {
    fn default() -> Self {
        Self::new()
    }
}
