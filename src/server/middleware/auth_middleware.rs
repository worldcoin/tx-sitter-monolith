use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderName, HeaderValue, Request};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use headers::Header;
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::server::ApiError;

pub const AUTHORIZED_RELAYER: &str = "x-authorized-relayer";
static HEADER_NAME: HeaderName = HeaderName::from_static(AUTHORIZED_RELAYER);

#[derive(Debug, Serialize, Deserialize)] // not sure if it works
pub enum AuthorizedRelayer {
    Named(String),
    Any,
}

impl AuthorizedRelayer {
    pub fn is_authorized(&self, relayer_id: &str) -> bool {
        match self {
            AuthorizedRelayer::Any => true,
            AuthorizedRelayer::Named(name) => name == relayer_id,
        }
    }
}

impl Header for AuthorizedRelayer {
    fn name() -> &'static HeaderName {
        &HEADER_NAME
    }

    fn decode<'i, I>(values: &mut I) -> Result<Self, headers::Error>
    where
        Self: Sized,
        I: Iterator<Item = &'i HeaderValue>,
    {
        let value = values.next().ok_or_else(headers::Error::invalid)?;
        let value = value
            .to_str()
            .map_err(|_| headers::Error::invalid())?
            .to_owned();

        if value == "*" {
            Ok(Self::Any)
        } else {
            Ok(Self::Named(value))
        }
    }

    fn encode<E: Extend<HeaderValue>>(&self, values: &mut E) {
        match self {
            AuthorizedRelayer::Named(name) => values
                .extend(std::iter::once(HeaderValue::from_str(name).unwrap())),
            AuthorizedRelayer::Any => {
                values.extend(std::iter::once(HeaderValue::from_static("*")))
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthParams {
    #[serde(default)]
    api_key: Option<String>,
}

pub async fn auth<B>(
    State(context): State<Arc<App>>,
    Query(query): Query<AuthParams>,
    request: Request<B>,
    next: Next<B>,
) -> Response {
    let (mut parts, body) = request.into_parts();

    if context.config.server.disable_auth {
        parts
            .headers
            .insert(AUTHORIZED_RELAYER, HeaderValue::from_str("*").unwrap());
    } else {
        let authorized_relayer = match auth_inner(context.clone(), query).await
        {
            Ok(relayer_id) => relayer_id,
            Err(error) => return error.into_response(),
        };

        parts.headers.insert(
            AUTHORIZED_RELAYER,
            HeaderValue::from_str(&authorized_relayer).unwrap(),
        );
    }

    let request = Request::from_parts(parts, body);

    next.run(request).await
}

async fn auth_inner(
    _app: Arc<App>,
    _query: AuthParams,
) -> Result<String, ApiError> {
    todo!("Add tables to DB and implement")
    // let mut api_key = None;

    // TODO: Support Bearer in auth header
    // let auth_header = parts.headers.get(AUTHORIZATION);
    // if let Some(auth_header) = auth_header {
    //     todo!()
    // }

    // if let Some(api_key_from_query) = query.api_key {
    //     api_key = Some(api_key_from_query);
    // }

    // let Some(api_key) = api_key else {
    //     return Err(ApiError::Unauthorized);
    // };

    // let api_key = hex::decode(&api_key).map_err(|err| {
    //     tracing::warn!(?err, "Error decoding api key");

    //     ApiError::KeyEncoding
    // })?;

    // let api_key: [u8; 32] =
    //     api_key.try_into().map_err(|_| ApiError::KeyLength)?;

    // let api_key_hash = Sha3_256::digest(&api_key);

    // let api_key_hash = hex::encode(api_key_hash);

    // // let relayer_id = context
    // //     .api_keys_db
    // //     .get_relayer_id_by_hash(api_key_hash)
    // //     .await?
    // //     .ok_or_else(|| ApiError::Unauthorized)?;

    // let relayer_id = todo!();

    // Ok(relayer_id)
}
