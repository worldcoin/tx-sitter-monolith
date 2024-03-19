use hyper::StatusCode;

pub mod network;
pub mod relayer;
pub mod transaction;

pub async fn health() -> StatusCode {
    StatusCode::OK
}
