use poem::{FromRequest, IntoResponse, Request, RequestBody, Response, Result};
use poem_openapi::error::{ContentTypeError, ParseRequestPayloadError};
use poem_openapi::payload::{Json, ParsePayload, Payload};
use poem_openapi::registry::{
    MetaMediaType, MetaRequest, MetaResponse, MetaResponses, MetaSchemaRef,
    Registry,
};
use poem_openapi::types::Type;
use poem_openapi::{
    ApiExtractor, ApiExtractorType, ApiResponse, ExtractParamOptions,
};
use serde::de::DeserializeOwned;
use serde::Serialize;

/// A JSON request or response body that serde reads and writes.
///
/// poem-openapi's [`Json`] converts bodies through [`serde_json::Value`],
/// which holds integers of at most 64 bits, so it changes JSON it passes on
/// unparsed, such as [`TxMetadata`](crate::types::TxMetadata). This payload
/// keeps it exact. `T`'s poem-openapi [`Type`] describes the body's schema,
/// and serde decides what is accepted, so the two must agree.
#[derive(Debug, Clone)]
pub struct ExactJson<T>(pub T);

impl<T: Type> Payload for ExactJson<T> {
    const CONTENT_TYPE: &'static str = Json::<T>::CONTENT_TYPE;

    fn check_content_type(content_type: &str) -> bool {
        Json::<T>::check_content_type(content_type)
    }

    fn schema_ref() -> MetaSchemaRef {
        T::schema_ref()
    }

    fn register(registry: &mut Registry) {
        T::register(registry);
    }
}

impl<T: Type + DeserializeOwned> ParsePayload for ExactJson<T> {
    const IS_REQUIRED: bool = true;

    async fn from_request(
        request: &Request,
        body: &mut RequestBody,
    ) -> Result<Self> {
        let data =
            <Vec<u8> as FromRequest>::from_request(request, body).await?;

        serde_json::from_slice(&data).map(Self).map_err(|err| {
            ParseRequestPayloadError {
                reason: err.to_string(),
            }
            .into()
        })
    }
}

impl<'a, T: Type + DeserializeOwned> ApiExtractor<'a> for ExactJson<T> {
    const TYPES: &'static [ApiExtractorType] =
        &[ApiExtractorType::RequestObject];

    type ParamType = ();
    type ParamRawType = ();

    fn register(registry: &mut Registry) {
        <Self as Payload>::register(registry);
    }

    fn request_meta() -> Option<MetaRequest> {
        Some(MetaRequest {
            description: None,
            content: vec![MetaMediaType {
                content_type: Self::CONTENT_TYPE,
                schema: Self::schema_ref(),
            }],
            required: Self::IS_REQUIRED,
        })
    }

    async fn from_request(
        request: &'a Request,
        body: &mut RequestBody,
        _param_opts: ExtractParamOptions<Self::ParamType>,
    ) -> Result<Self> {
        let content_type = request
            .content_type()
            .ok_or(ContentTypeError::ExpectContentType)?;

        if !Self::check_content_type(content_type) {
            return Err(ContentTypeError::NotSupported {
                content_type: content_type.to_string(),
            }
            .into());
        }

        <Self as ParsePayload>::from_request(request, body).await
    }
}

impl<T: Type + Serialize + Send> IntoResponse for ExactJson<T> {
    fn into_response(self) -> Response {
        poem::web::Json(self.0).into_response()
    }
}

impl<T: Type + Serialize + Send> ApiResponse for ExactJson<T> {
    fn meta() -> MetaResponses {
        MetaResponses {
            responses: vec![MetaResponse {
                description: "",
                status: Some(200),
                content: vec![MetaMediaType {
                    content_type: Self::CONTENT_TYPE,
                    schema: Self::schema_ref(),
                }],
                headers: vec![],
            }],
        }
    }

    fn register(registry: &mut Registry) {
        T::register(registry);
    }
}
