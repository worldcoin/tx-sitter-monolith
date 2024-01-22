use axum::extract::MatchedPath;
use hyper::Request;
use tower_http::trace::MakeSpan;
use tracing::{Level, Span};

/// MakeSpan to remove api keys from logs
#[derive(Clone)]
pub(crate) struct MatchedPathMakeSpan;

impl<B> MakeSpan<B> for MatchedPathMakeSpan {
    fn make_span(&mut self, request: &Request<B>) -> Span {
        let matched_path = request
            .extensions()
            .get::<MatchedPath>()
            .map(MatchedPath::as_str);

        tracing::span!(
            Level::DEBUG,
            "request",
            method = %request.method(),
            matched_path,
            version = ?request.version(),
        )
    }
}
