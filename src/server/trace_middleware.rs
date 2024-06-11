use poem::http::uri::Builder;
use poem::{Endpoint, IntoResponse, Middleware, Request, Response, Result};
use tracing::{Instrument, Level};

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
        let obfuscated_uri_path = if req.uri().path().starts_with("/1/api") {
            // Regex to hide the api token in a url like /1/api/<API_TOKEN>/...
            let re = regex::Regex::new(r"/1/api/([^/]+)/").unwrap();
            re.replace(req.uri().path(), "/1/api/.../")
        } else {
            req.uri().path().into()
        };

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

        let span = tracing::span!(Level::DEBUG, "request", method = %req.method(), uri = %req_uri);

        let res = async move {
            // TODO: Propagate span from request headers
            tracing::debug!("started processing request");

            let res = self.0.call(req).await;
            let response = match res {
                Ok(r) => r.into_response(),
                Err(err) => {
                    let stacktrace = format!("{:?}", err);
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

            response
        }.instrument(span).await;

        Ok(res)
    }
}
