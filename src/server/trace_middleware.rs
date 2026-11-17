use poem::http::uri::Builder;
use poem::{Endpoint, IntoResponse, Middleware, Request, Response, Result};
use telemetry_batteries::tracing::{trace_from_headers, trace_to_headers};
use tracing::{Instrument, Level};

use crate::api_key::ApiKey;

pub struct TraceMiddleware;

impl<E: Endpoint> Middleware<E> for TraceMiddleware {
    type Output = TraceMiddlwareImpl<E>;

    fn transform(&self, ep: E) -> Self::Output {
        TraceMiddlwareImpl(ep)
    }
}

pub struct TraceMiddlwareImpl<E>(E);

impl<E: Endpoint> Endpoint for TraceMiddlwareImpl<E> {
    type Output = Response;

    async fn call(&self, req: Request) -> Result<Self::Output> {
        let obfuscated_uri_path = redact_api_keys(req.uri().path());

        let mut req_uri_builder = Builder::from(req.uri().clone());
        if let Some(path_and_query) = req.uri().path_and_query() {
            let q = path_and_query.query();

            let new_p_and_q = if let Some(q) = q {
                format!("{obfuscated_uri_path}?{q}")
            } else {
                obfuscated_uri_path.to_string()
            };

            req_uri_builder = req_uri_builder.path_and_query(new_p_and_q);
        }

        let req_uri = req_uri_builder.build().expect("Invalid URI");

        let span = tracing::span!(Level::INFO, "request", method = %req.method(), uri = %req_uri);

        let res = async move {
            trace_from_headers(req.headers());

            tracing::debug!("started processing request");

            let res = self.0.call(req).await;
            let mut response = match res {
                Ok(r) => r.into_response(),
                Err(err) => {
                    let stacktrace = format!("{err:?}");
                    let message = err.to_string();

                    tracing::error!(error.message = message, error.stack = stacktrace, error.kind = "Error", "error processing request");

                    err.into_response()
                },
            };

            if response.status().is_server_error() {
                tracing::error!(status = %response.status(), "finished processing request");
            } else if response.status().is_client_error() {
                tracing::warn!(status = %response.status(), "finished processing request");
            } else {
                tracing::debug!(status = %response.status(), "finished processing request");
            }

            trace_to_headers(response.headers_mut());

            response
        }.instrument(span).await;

        Ok(res)
    }
}

/// Returns `path` with every segment that is an API key replaced by `...`.
///
/// That covers the segment where the v1 API takes its key, whatever it holds,
/// and any segment that parses as a key, such as one sent to a v2 path by
/// mistake.
fn redact_api_keys(path: &str) -> String {
    let v1_api_key_segment = path.starts_with("/1/api/").then_some(3);

    path.split('/')
        .enumerate()
        .map(|(index, segment)| {
            let is_api_key = Some(index) == v1_api_key_segment
                || segment
                    .replace("%3D", "=")
                    .replace("%3d", "=")
                    .parse::<ApiKey>()
                    .is_ok();

            if is_api_key {
                "..."
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    const RELAYER_ID: &str = "1b908a34-5dc1-4d2d-a146-5eb46e975830";

    #[test_case("/1/api/{key}/tx", "/1/api/.../tx" ; "v1 key")]
    #[test_case("/1/api/not-a-key/tx", "/1/api/.../tx" ; "v1 malformed key")]
    #[test_case("/2/{key}/tx", "/2/.../tx" ; "key in a v2 path")]
    #[test_case("/2/tx/{key}", "/2/tx/..." ; "key as a tx id")]
    #[test_case("/2/{encoded_key}/tx", "/2/.../tx" ; "percent encoded key")]
    #[test_case("/2/tx/tx-1", "/2/tx/tx-1" ; "no key")]
    #[test_case("/1/admin/relayer/{relayer_id}", "/1/admin/relayer/{relayer_id}" ; "relayer id")]
    fn redacts_api_keys(path: &str, expected: &str) {
        let key = ApiKey::random(RELAYER_ID).reveal().unwrap();
        assert!(key.ends_with('='), "{key}");
        let encoded_key = key.replace('=', "%3D");
        let fill = |template: &str| {
            template
                .replace("{key}", &key)
                .replace("{encoded_key}", &encoded_key)
                .replace("{relayer_id}", RELAYER_ID)
        };

        assert_eq!(redact_api_keys(&fill(path)), fill(expected));
    }
}
