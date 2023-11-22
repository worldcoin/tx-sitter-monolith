use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use hyper::Body;

pub async fn log_response<B>(request: Request<B>, next: Next<B>) -> Response {
    let mut response = next.run(request).await;

    if !response.status().is_success() {
        let body_bytes = hyper::body::to_bytes(response.body_mut())
            .await
            .expect("Failed to read body");

        let body_as_text = std::str::from_utf8(&body_bytes)
            .unwrap_or("Failed to parse body as text");

        let status_code = response.status();
        tracing::error!(?status_code, "{body_as_text}");

        *response.body_mut() = axum::body::boxed(Body::from(body_bytes));
    }

    response
}
