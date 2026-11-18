use std::borrow::Cow;
use std::fmt;
use std::ops::RangeInclusive;
use std::sync::Arc;
use std::time::Duration;

use eyre::Context;

use poem::error::ResponseError;
use poem::http::{header, StatusCode};
use poem::web::Data;
use poem::{Endpoint, EndpointExt, IntoResponse, Request, Response};
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::{Json, Payload};
use poem_openapi::registry::{
    MetaHeader, MetaMediaType, MetaResponse, MetaResponses, Registry,
};
use poem_openapi::types::Type;
use poem_openapi::{ApiResponse, OpenApi};
use rand::Rng;

use super::exact_json::ExactJson;
use super::security::BearerApiKey;
use super::{
    ensure_relayer_accepts_tx, parse_gas_limit, OpenAPITags, SendTxError,
};
use crate::app::App;
use crate::db::{CreateResult, TxEventsPage, TxIdMatch};
use crate::tx_events::{EventCursor, EventNotifier, TooManyWaiters};
use crate::types::{
    ErrorResponse, ErrorResponseBody, GetTxEventsResponse, GetTxResponseV2,
    SendTxRequestV2, SendTxResponse,
};

/// The largest `POST /2/tx` body: room for the largest transaction nodes
/// relay (128 KiB of calldata, hex encoded) plus the metadata
const MAX_SEND_TX_BODY_SIZE: usize = 512 * 1024;

/// When a request refused with `too_many_waiters` may try again, in
/// seconds, spread so that refused requests don't all return at once
const WAITER_RETRY_AFTER_SECS: RangeInclusive<u64> = 1..=3;

pub struct RelayerApiV2;

#[OpenApi(prefix_path = "/2/")]
impl RelayerApiV2 {
    /// Send Transaction
    ///
    /// Sending a `txId` again with an identical request returns the same
    /// `txId` and changes nothing, even while the relayer is disabled or its
    /// queue is full. Sending it with a different request returns 409.
    #[oai(
        path = "/tx",
        method = "post",
        operation_id = "create_transaction_v2",
        tag = "OpenAPITags::RelayerV2",
        transform = "send_tx_endpoint"
    )]
    async fn send_tx(
        &self,
        api_key: BearerApiKey,
        Data(app): Data<&Arc<App>>,
        ExactJson(req): ExactJson<SendTxRequestV2>,
    ) -> Result<ExactJson<SendTxResponse>, ApiError> {
        let api_key = api_key.authorize(app).await?;
        let relayer_id = api_key.relayer_id();

        let gas_limit = parse_gas_limit(&req.gas_limit)?;
        let request_hash = req.request_hash(relayer_id);
        let tx_id = req
            .tx_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let relayer =
            match ensure_relayer_accepts_tx(app, relayer_id, gas_limit).await {
                Ok(relayer) => relayer,
                // A refused send that repeats a stored one is answered like it
                Err(err) => {
                    return match app
                        .db
                        .compare_tx_request(&tx_id, relayer_id, request_hash)
                        .await?
                    {
                        TxIdMatch::Unused => Err(err.into()),
                        TxIdMatch::SameRequest => Ok(repeated_send(tx_id)),
                        TxIdMatch::DifferentRequest => Err(conflicting_send()),
                    };
                }
            };

        let result = app
            .db
            .create_transaction(
                &tx_id,
                req.to.0,
                req.data.as_ref().map(|d| &d.0[..]).unwrap_or(&[]),
                req.value.0,
                gas_limit,
                req.priority,
                None,
                relayer_id,
                req.metadata.as_ref(),
                Some(request_hash),
            )
            .await?;

        match result {
            CreateResult::Created => Ok(ExactJson(SendTxResponse { tx_id })),
            CreateResult::Repeat => Ok(repeated_send(tx_id)),
            CreateResult::Conflict => Err(conflicting_send()),
            CreateResult::QueueFull => Err(SendTxError::QueueFull.into()),
            CreateResult::NonceTaken { nonce } => {
                Err(SendTxError::nonce_taken(&relayer, &tx_id, nonce).into())
            }
        }
    }

    /// Get Transaction
    ///
    /// Returns 404 for a transaction of another relayer.
    #[oai(
        path = "/tx/:tx_id",
        method = "get",
        operation_id = "get_transaction_v2",
        tag = "OpenAPITags::RelayerV2",
        transform = "api_errors"
    )]
    async fn get_tx(
        &self,
        api_key: BearerApiKey,
        Data(app): Data<&Arc<App>>,
        #[oai(validator(pattern = r"^[^\x00]*$"))] Path(tx_id): Path<String>,
    ) -> Result<ExactJson<GetTxResponseV2>, ApiError> {
        let api_key = api_key.authorize(app).await?;

        let tx = app
            .db
            .read_relayer_tx(api_key.relayer_id(), &tx_id)
            .await?
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    "Transaction not found",
                )
            })?;

        Ok(ExactJson(tx.into()))
    }

    /// Get Transaction Events
    ///
    /// Returns the status changes of the relayer's transactions in the order
    /// they happened, at most `limit` of them. Pass `nextCursor` as `after`
    /// to get the ones that follow. An event is returned once every
    /// database transaction that started writing before it has finished, so
    /// no event is stored later before one already returned.
    ///
    /// With `wait`, a request that finds no events waits up to that many
    /// seconds for one and then returns the events stored by then, possibly
    /// none.
    ///
    /// Events are kept for `service.event_retention` (7 days by default). A
    /// cursor older than that gets 410, because events after it may be gone,
    /// and so does a cursor from before a restore of the database.
    #[oai(
        path = "/events",
        method = "get",
        operation_id = "get_events_v2",
        tag = "OpenAPITags::RelayerV2",
        transform = "api_errors"
    )]
    async fn get_events(
        &self,
        api_key: BearerApiKey,
        Data(app): Data<&Arc<App>>,
        Data(notifier): Data<&Arc<EventNotifier>>,
        /// The `nextCursor` of an earlier response of the relayer's feed.
        /// Without it the feed starts at the oldest event kept.
        Query(after): Query<Option<String>>,
        /// The most events to return
        #[oai(
            default = "default_events_limit",
            validator(minimum(value = "1"), maximum(value = "1000"))
        )]
        Query(limit): Query<u32>,
        /// How many seconds to wait for an event while there are none
        #[oai(default, validator(maximum(value = "30")))]
        Query(wait): Query<u64>,
    ) -> Result<ExactJson<GetTxEventsResponse>, EventsApiError> {
        let api_key = api_key.authorize(app).await?;
        let relayer_id = api_key.relayer_id();
        let after = after
            .map(|after| EventCursor::decode(&after, relayer_id))
            .transpose()
            .map_err(|err| {
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    err.to_string(),
                )
            })?;

        let mut poll = notifier
            .long_poll(relayer_id, Duration::from_secs(wait))
            .map_err(ApiError::from)?;
        loop {
            let page = tokio::time::timeout_at(
                poll.read_deadline(),
                app.db.read_tx_events(relayer_id, after, limit),
            )
            .await
            .context("Reading tx events timed out")??;

            let (events, next_cursor) = match page {
                TxEventsPage::Events {
                    events,
                    next_cursor,
                } => (events, next_cursor),
                TxEventsPage::CursorExpired => {
                    return Err(cursor_expired(
                        "Events after the cursor were deleted",
                    ))
                }
                TxEventsPage::CursorAhead => {
                    tracing::warn!(
                        relayer_id,
                        ?after,
                        "Refused a cursor ahead of the database"
                    );
                    return Err(cursor_expired(
                        "The cursor is ahead of the database, which was restored or replaced since",
                    ));
                }
            };

            if !events.is_empty() || !poll.wait().await {
                return Ok(ExactJson(GetTxEventsResponse::new(
                    events,
                    next_cursor,
                    relayer_id,
                )));
            }
        }
    }
}

fn default_events_limit() -> u32 {
    100
}

fn cursor_expired(reason: &str) -> EventsApiError {
    ApiError::new(
        StatusCode::GONE,
        "cursor_expired",
        format!("{reason}. Reconcile from the transactions and restart without a cursor"),
    )
    .into()
}

fn repeated_send(tx_id: String) -> ExactJson<SendTxResponse> {
    metrics::counter!("repeated_tx_sends", "outcome" => "same_request")
        .increment(1);

    ExactJson(SendTxResponse { tx_id })
}

fn conflicting_send() -> ApiError {
    metrics::counter!("repeated_tx_sends", "outcome" => "different_request")
        .increment(1);

    SendTxError::TxIdConflict.into()
}

fn send_tx_endpoint(ep: impl Endpoint + 'static) -> impl Endpoint {
    api_errors(ep.before(|mut req: Request| async move {
        let body = req
            .take_body()
            .into_bytes_limit(MAX_SEND_TX_BODY_SIZE)
            .await?;
        req.set_body(body);

        Ok(req)
    }))
}

/// Answers every error of a v2 operation with an [`ApiError`], including
/// the ones poem-openapi raises before the operation runs, such as for a
/// missing API key or an invalid body, and counts them by `errorId`
fn api_errors(ep: impl Endpoint + 'static) -> impl Endpoint {
    ep.around(|ep, req| async move {
        ep.call(req)
            .await
            .map(IntoResponse::into_response)
            .map_err(|err| {
                let err = ApiError::from(err);
                metrics::counter!("v2_api_errors", "error_id" => err.error_id)
                    .increment(1);

                err.into()
            })
    })
}

/// An error of the v2 API.
///
/// It responds with an [`ErrorResponseBody`]: `errorId` names the error and
/// stays stable, `errorMessage` describes it for people. Internal errors
/// keep their cause out of the response, and in their [`Display`](fmt::Display)
/// for the logs.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    error_id: &'static str,
    message: Cow<'static, str>,
    cause: Option<eyre::Report>,
    /// Sent as the `Retry-After` header
    retry_after: Option<Duration>,
}

impl ApiError {
    fn new(
        status: StatusCode,
        error_id: &'static str,
        message: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            status,
            error_id,
            message: message.into(),
            cause: None,
            retry_after: None,
        }
    }

    fn with_cause(mut self, cause: eyre::Report) -> Self {
        self.cause = Some(cause);
        self
    }

    fn internal(cause: eyre::Report) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Internal server error",
        )
        .with_cause(cause)
    }
}

impl From<SendTxError> for ApiError {
    fn from(err: SendTxError) -> Self {
        match err {
            SendTxError::RelayerDisabled => Self::new(
                StatusCode::FORBIDDEN,
                "relayer_disabled",
                "Relayer is disabled",
            ),
            SendTxError::QueueFull => Self::new(
                StatusCode::TOO_MANY_REQUESTS,
                "queue_full",
                "Relayer queue is full",
            ),
            SendTxError::InsufficientFunds => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "insufficient_funds",
                "Relayer funds are insufficient for its queued transactions and this one",
            ),
            SendTxError::TxIdConflict => Self::new(
                StatusCode::CONFLICT,
                "transaction_already_exists",
                "A transaction with this id already exists for a different request",
            ),
            SendTxError::NonceTaken => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "relayer_nonce_taken",
                "The relayer's next nonce is taken by another of its transactions. Resetting the relayer repairs it.",
            ),
            SendTxError::BalanceUnavailable(cause) => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "balance_unavailable",
                "The relayer's balance couldn't be checked, retry later",
            )
            .with_cause(cause),
            SendTxError::Internal(cause) => Self::internal(cause),
        }
    }
}

impl From<TooManyWaiters> for ApiError {
    fn from(_: TooManyWaiters) -> Self {
        let retry_after = rand::thread_rng().gen_range(WAITER_RETRY_AFTER_SECS);

        Self {
            retry_after: Some(Duration::from_secs(retry_after)),
            ..Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "too_many_waiters",
                "Too many requests are waiting for events, retry later",
            )
        }
    }
}

impl From<eyre::Report> for ApiError {
    fn from(err: eyre::Report) -> Self {
        Self::internal(err)
    }
}

/// Keeps an [`ApiError`] as it is, and names any other error by its status
impl From<poem::Error> for ApiError {
    fn from(err: poem::Error) -> Self {
        let err = match err.downcast::<ApiError>() {
            Ok(api_error) => return api_error,
            Err(err) => err,
        };

        let status = err.status();
        match status {
            StatusCode::UNAUTHORIZED => {
                Self::new(status, "unauthorized", "Unauthorized")
            }
            StatusCode::NOT_FOUND => {
                Self::new(status, "not_found", err.to_string())
            }
            StatusCode::PAYLOAD_TOO_LARGE => {
                Self::new(status, "payload_too_large", err.to_string())
            }
            StatusCode::UNSUPPORTED_MEDIA_TYPE => {
                Self::new(status, "unsupported_media_type", err.to_string())
            }
            status if status.is_server_error() => {
                Self::internal(eyre::eyre!("{err}"))
            }
            _ => Self::new(status, "invalid_request", err.to_string()),
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.cause {
            Some(cause) => write!(f, "{}: {cause:#}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for ApiError {}

impl ResponseError for ApiError {
    fn status(&self) -> StatusCode {
        self.status
    }

    fn as_response(&self) -> Response {
        let mut response = ErrorResponse::new(
            self.status,
            self.error_id,
            self.message.clone(),
        )
        .into_response();

        if let Some(retry_after) = self.retry_after {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, retry_after.as_secs().into());
        }

        response
    }
}

/// The responses an [`ApiError`] of the transaction operations can be, with
/// the `errorId`s of each
const ERROR_RESPONSES: &[(StatusCode, &str)] = &[
    (
        StatusCode::BAD_REQUEST,
        "`invalid_request`: the request is invalid",
    ),
    (
        StatusCode::UNAUTHORIZED,
        "`unauthorized`: the bearer API key is missing or unknown",
    ),
    (
        StatusCode::FORBIDDEN,
        "`relayer_disabled`: the relayer is disabled",
    ),
    (
        StatusCode::NOT_FOUND,
        "`not_found`: the relayer has no transaction with this id",
    ),
    (
        StatusCode::CONFLICT,
        "`transaction_already_exists`: a different request already used this `txId`",
    ),
    (
        StatusCode::PAYLOAD_TOO_LARGE,
        "`payload_too_large`: the request body is over 512 KiB",
    ),
    (
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "`unsupported_media_type`: the request body isn't JSON",
    ),
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        "`insufficient_funds`: the relayer can't pay for its queued transactions and this one",
    ),
    (
        StatusCode::TOO_MANY_REQUESTS,
        "`queue_full`: the relayer's queue is full",
    ),
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "`internal_error`, or `relayer_nonce_taken` until the relayer is reset",
    ),
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "`balance_unavailable`: the relayer's balance couldn't be checked, retry later",
    ),
];

impl ApiResponse for ApiError {
    fn meta() -> MetaResponses {
        let responses = ERROR_RESPONSES
            .iter()
            .map(|&(status, description)| error_response(status, description))
            .collect();

        MetaResponses { responses }
    }

    fn register(registry: &mut Registry) {
        ErrorResponseBody::register(registry);
    }
}

/// An [`ApiError`] of `GET /2/events`, which the OpenAPI schema declares
/// with that operation's responses only
#[derive(Debug)]
pub struct EventsApiError(ApiError);

impl From<ApiError> for EventsApiError {
    fn from(err: ApiError) -> Self {
        Self(err)
    }
}

impl From<eyre::Report> for EventsApiError {
    fn from(err: eyre::Report) -> Self {
        Self(err.into())
    }
}

impl From<poem::Error> for EventsApiError {
    fn from(err: poem::Error) -> Self {
        Self(err.into())
    }
}

impl From<EventsApiError> for poem::Error {
    fn from(err: EventsApiError) -> Self {
        err.0.into()
    }
}

/// The responses an [`EventsApiError`] can be, with the `errorId`s of each
const EVENTS_ERROR_RESPONSES: &[(StatusCode, &str)] = &[
    (
        StatusCode::BAD_REQUEST,
        "`invalid_request`: the request is invalid, or `after` isn't a cursor of the relayer's feed",
    ),
    (
        StatusCode::UNAUTHORIZED,
        "`unauthorized`: the bearer API key is missing or unknown",
    ),
    (
        StatusCode::GONE,
        "`cursor_expired`: the feed may miss events after the cursor. Reconcile from the transactions and restart without a cursor",
    ),
    (StatusCode::INTERNAL_SERVER_ERROR, "`internal_error`"),
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "`too_many_waiters`: too many requests wait for events, retry after `Retry-After` seconds",
    ),
];

impl ApiResponse for EventsApiError {
    fn meta() -> MetaResponses {
        let responses = EVENTS_ERROR_RESPONSES
            .iter()
            .map(|&(status, description)| {
                let mut response = error_response(status, description);
                if status == StatusCode::SERVICE_UNAVAILABLE {
                    response.headers.push(retry_after_header());
                }

                response
            })
            .collect();

        MetaResponses { responses }
    }

    fn register(registry: &mut Registry) {
        ApiError::register(registry);
    }
}

fn error_response(
    status: StatusCode,
    description: &'static str,
) -> MetaResponse {
    MetaResponse {
        description,
        status: Some(status.as_u16()),
        content: vec![MetaMediaType {
            content_type: Json::<ErrorResponseBody>::CONTENT_TYPE,
            schema: ErrorResponseBody::schema_ref(),
        }],
        headers: vec![],
    }
}

fn retry_after_header() -> MetaHeader {
    MetaHeader {
        name: header::RETRY_AFTER.to_string(),
        description: Some("Seconds to wait before retrying".to_string()),
        required: true,
        deprecated: false,
        schema: u64::schema_ref(),
    }
}
