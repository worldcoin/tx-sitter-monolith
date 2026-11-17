mod common;

use crate::common::prelude::*;

#[tokio::test]
async fn readiness_fails_while_database_is_unreachable() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (service, _) = ServiceBuilder::default()
        .role(ServiceRole::Api)
        .build(&anvil, &db_url)
        .await?;
    let addr = service.local_addr();

    let (status, _) = http_get(addr, "/health/ready").await?;
    assert_eq!(status, StatusCode::OK);

    drop(db_container);

    let (status, body) = http_get(addr, "/health/ready").await?;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body.contains("Database is unreachable"), "{body}");

    let (status, _) = http_get(addr, "/health/live").await?;
    assert_eq!(status, StatusCode::OK, "liveness must not depend on the DB");

    Ok(())
}
