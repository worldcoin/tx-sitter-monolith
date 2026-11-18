mod common;

use std::collections::HashMap;

use eyre::ContextCompat;
use serde_json::value::RawValue;
use serde_json::Value;
use tokio::time::Instant;
use tx_sitter::db::Database;
use tx_sitter_client::apis::relayer_v2_api::{
    GetEventsV2Error, GetEventsV2Params,
};
use tx_sitter_client::apis::Error;

use crate::common::prelude::*;

const METADATA: &str =
    r#"{ "transactionType": "Transfer", "amount": 18446744073709551617 }"#;

#[tokio::test]
async fn events_follow_a_tx_through_a_reorg() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    let anvil_port = anvil.port();
    let (service, client) = ServiceBuilder::default()
        .hard_reorg_interval(Duration::from_secs(2))
        .build(&anvil, &db_url)
        .await?;
    let api_key = DEFAULT_API_KEY.to_string();
    let base_url = format!("http://{}", service.local_addr());
    let value: U256 = parse_units("1", "ether")?.into();
    send_raw_tx_v2(&base_url, &api_key, "tx-1", value).await?;

    let (mut events, cursor) = await_events(
        &base_url,
        &api_key,
        None,
        &[TxStatus::Pending, TxStatus::Mined],
    )
    .await?;
    let provider = setup_provider(anvil.endpoint()).await?;
    await_balance(&provider, value, ARBITRARY_ADDRESS).await?;

    tracing::warn!("Dropping anvil & restarting at port {anvil_port}");
    drop(anvil);
    let anvil = AnvilBuilder::default().port(anvil_port).spawn().await?;
    let (reorg_events, _) = await_events(
        &base_url,
        &api_key,
        cursor,
        &[TxStatus::Pending, TxStatus::Mined],
    )
    .await?;
    events.extend(reorg_events);
    let provider = setup_provider(anvil.endpoint()).await?;
    await_balance(&provider, value, ARBITRARY_ADDRESS).await?;

    let statuses: Vec<_> = events.iter().map(status).collect();
    let mut changes = statuses.clone();
    changes.dedup();
    assert_eq!(
        changes,
        [
            TxStatus::Pending,
            TxStatus::Mined,
            TxStatus::Pending,
            TxStatus::Mined
        ],
        "{statuses:?}"
    );
    for event in &events {
        assert_eq!(event["txId"].get(), r#""tx-1""#);
        assert_eq!(
            event["relayerId"].get(),
            format!(r#""{DEFAULT_RELAYER_ID}""#)
        );
        assert_eq!(event["chainId"].get(), DEFAULT_ANVIL_CHAIN_ID.to_string());
        assert_eq!(event["metadata"].get(), METADATA);
    }
    let mined = events.last().expect("Events were awaited");
    let v2_client = Configuration {
        bearer_access_token: Some(api_key.clone()),
        ..client
    };
    let tx = get_tx_v2(&v2_client, "tx-1").await?;
    let tx_hash: base_api_types::H256 =
        serde_json::from_str(mined["txHash"].get())?;
    assert_eq!(tx.tx_hash.as_ref(), Some(&tx_hash));
    let receipt = provider.get_transaction_receipt(tx_hash.0).await?;
    assert!(receipt.is_some(), "{tx_hash:?} isn't mined after the reorg");

    Ok(())
}

#[tokio::test]
async fn long_poll_wakes_on_a_new_event() -> eyre::Result<()> {
    setup_tracing();

    let service = TestService::spawn(api_service()).await?;
    let client = service.v2_client(&service.api_key);
    send_tx_v2(&client, send_tx_request("tx-1")).await?;

    let (events, stored_at) =
        tokio::join!(get_events(&client, None, None, Some(10)), async {
            // Late enough for the poll to be waiting. A missed notification
            // would only be noticed by the recheck a second after the poll's
            // first read.
            tokio::time::sleep(Duration::from_millis(500)).await;
            broadcast(&service.db, "tx-1", 1).await?;

            Ok::<_, eyre::Report>(Instant::now())
        });
    let woken_after = stored_at?.elapsed();
    let events = events?;

    assert_eq!(events.events.len(), 1);
    assert_eq!(events.events[0].tx_id, "tx-1");
    assert_eq!(events.events[0].status, TxStatus::Pending);
    assert!(
        woken_after < Duration::from_millis(400),
        "Woken {woken_after:?} after the event was stored"
    );

    Ok(())
}

#[tokio::test]
async fn long_poll_without_events_returns_at_its_deadline() -> eyre::Result<()>
{
    setup_tracing();

    let service = TestService::spawn(api_service()).await?;
    let client = service.v2_client(&service.api_key);

    let started = Instant::now();
    let events = get_events(&client, None, None, Some(2)).await?;
    let waited = started.elapsed();

    assert_eq!(events.events, []);
    assert!(waited >= Duration::from_secs(2), "Waited {waited:?}");
    assert!(waited < Duration::from_secs(4), "Waited {waited:?}");

    // The cursor of a page without events still leads to the next one
    send_tx_v2(&client, send_tx_request("tx-1")).await?;
    broadcast(&service.db, "tx-1", 1).await?;
    let broadcast_page = get_events(&client, None, None, None).await?;
    let empty_page =
        get_events(&client, Some(broadcast_page.next_cursor), None, Some(1))
            .await?;
    assert_eq!(empty_page.events, []);
    escalate(&service.db, "tx-1", 2).await?;
    let escalation_page =
        get_events(&client, Some(empty_page.next_cursor), None, None).await?;
    assert_eq!(tx_hashes(&escalation_page), [B256::with_last_byte(2)]);

    Ok(())
}

#[tokio::test]
async fn long_polls_return_once_shutdown_is_requested() -> eyre::Result<()> {
    setup_tracing();

    let shutdown = CancellationToken::new();
    let service =
        TestService::spawn(api_service().shutdown(shutdown.clone())).await?;
    let client = service.v2_client(&service.api_key);

    let poll =
        tokio::spawn(
            async move { get_events(&client, None, None, Some(30)).await },
        );
    tokio::time::sleep(Duration::from_millis(500)).await;
    shutdown.cancel();
    let events = tokio::time::timeout(Duration::from_secs(5), poll).await???;

    assert_eq!(events.events, []);

    Ok(())
}

#[tokio::test]
async fn cursor_before_pruned_events_gets_410() -> eyre::Result<()> {
    setup_tracing();

    let service = TestService::spawn(api_service()).await?;
    let client = service.v2_client(&service.api_key);
    send_tx_v2(&client, send_tx_request("tx-1")).await?;
    broadcast(&service.db, "tx-1", 1).await?;
    escalate(&service.db, "tx-1", 2).await?;
    let first = get_events(&client, None, Some(1), None).await?;

    sqlx::query("UPDATE tx_events SET created_at = now() - interval '8 days'")
        .execute(&service.db.pool)
        .await?;
    let prune_timestamp = chrono::Utc::now() - chrono::Duration::days(7);
    assert_eq!(service.db.prune_tx_events(prune_timestamp, 100).await?, 2);

    let error = expect_v2_error(
        get_events(&client, Some(first.next_cursor), Some(1), None).await,
        StatusCode::GONE,
    )?;
    assert_eq!(error.error_id, "cursor_expired");
    assert_eq!(get_events(&client, None, None, None).await?.events, []);

    Ok(())
}

#[tokio::test]
async fn relayers_see_only_their_events() -> eyre::Result<()> {
    setup_tracing();

    let service = TestService::spawn(api_service()).await?;
    let client = service.v2_client(&service.api_key);
    let (other_relayer_id, other_api_key) =
        service.create_relayer_with_api_key().await?;
    let other_client = service.v2_client(&other_api_key);
    send_tx_v2(&client, send_tx_request("tx-1")).await?;
    send_tx_v2(&other_client, send_tx_request("other-tx-1")).await?;
    send_tx_v2(&client, send_tx_request("tx-2")).await?;
    for (tx_id, hash_byte) in [("tx-1", 1), ("other-tx-1", 2), ("tx-2", 3)] {
        broadcast(&service.db, tx_id, hash_byte).await?;
    }

    let events = get_events(&client, None, None, None).await?;
    let other_events = get_events(&other_client, None, None, None).await?;

    let tx_ids = |events: &GetTxEventsResponse| -> Vec<String> {
        events.events.iter().map(|e| e.tx_id.clone()).collect()
    };
    assert_eq!(tx_ids(&events), ["tx-1", "tx-2"]);
    assert_eq!(tx_ids(&other_events), ["other-tx-1"]);
    assert!(other_events
        .events
        .iter()
        .all(|event| event.relayer_id == other_relayer_id));

    // A cursor is a position in its relayer's feed only
    let first = get_events(&client, None, Some(1), None).await?;
    let error = expect_v2_error(
        get_events(&other_client, Some(first.next_cursor), None, None).await,
        StatusCode::BAD_REQUEST,
    )?;
    assert_eq!(error.error_id, "invalid_request");

    Ok(())
}

#[tokio::test]
async fn polls_beyond_max_event_waiters_get_503() -> eyre::Result<()> {
    setup_tracing();

    let service =
        TestService::spawn(api_service().max_event_waiters(1)).await?;
    let client = service.v2_client(&service.api_key);

    let mut polls: FuturesUnordered<_> = (0..3)
        .map(|_| get_events(&client, None, None, Some(3)))
        .collect();
    for _ in 0..2 {
        let refused = polls.next().await.context("Three polls were sent")?;
        let error = expect_v2_error(refused, StatusCode::SERVICE_UNAVAILABLE)?;
        assert_eq!(error.error_id, "too_many_waiters");
    }

    // The third poll still waits in the only slot
    let response = reqwest::Client::new()
        .get(format!("{}/2/events?wait=1", service.base_url()))
        .bearer_auth(&service.api_key)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let retry_after: u64 =
        response.headers()["retry-after"].to_str()?.parse()?;
    assert!((1..=3).contains(&retry_after), "Retry-After: {retry_after}");

    let waited = polls.next().await.context("Three polls were sent")??;
    assert_eq!(waited.events, []);
    get_events(&client, None, None, Some(1)).await?;
    get_events(&client, None, None, None).await?;

    Ok(())
}

#[tokio::test]
async fn events_reject_invalid_requests() -> eyre::Result<()> {
    setup_tracing();

    let service = TestService::spawn(api_service()).await?;
    let http = reqwest::Client::new();
    let url = format!("{}/2/events", service.base_url());

    for query in [
        "after=not-a-cursor",
        "after=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "limit=0",
        "limit=1001",
        "wait=31",
        "wait=1s",
    ] {
        let response = http
            .get(format!("{url}?{query}"))
            .bearer_auth(&service.api_key)
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{query}");
        let error: ErrorResponseBody = response.json().await?;
        assert_eq!(error.error_id, "invalid_request", "{query}");
    }

    let response = http.get(&url).send().await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    Ok(())
}

#[tokio::test]
async fn soft_reorg_event_carries_the_new_hash() -> eyre::Result<()> {
    setup_tracing();

    let service = TestService::spawn(api_service()).await?;
    let client = service.v2_client(&service.api_key);
    send_tx_v2(&client, send_tx_request("tx-1")).await?;
    broadcast(&service.db, "tx-1", 1).await?;
    escalate(&service.db, "tx-1", 2).await?;

    // The first broadcast is mined, and then a reorg mines the escalation
    let chain_id = DEFAULT_ANVIL_CHAIN_ID;
    let block_txs = [B256::with_last_byte(1)];
    service
        .db
        .save_block(1, chain_id, chrono::Utc::now(), &block_txs)
        .await?;
    service.db.mine_txs(chain_id).await?;
    let reorged_txs = [B256::with_last_byte(2)];
    service
        .db
        .save_block(1, chain_id, chrono::Utc::now(), &reorged_txs)
        .await?;
    assert_eq!(service.db.handle_soft_reorgs().await?, ["tx-1"]);

    let page = get_events(&client, None, None, None).await?;
    let changes: Vec<_> = page
        .events
        .iter()
        .map(|event| (event.status, event.tx_hash.as_ref().map(|h| h.0)))
        .collect();
    assert_eq!(
        changes,
        [
            (TxStatus::Pending, Some(B256::with_last_byte(1))),
            (TxStatus::Pending, Some(B256::with_last_byte(2))),
            (TxStatus::Mined, Some(B256::with_last_byte(1))),
            (TxStatus::Mined, Some(B256::with_last_byte(2))),
        ]
    );

    Ok(())
}

#[tokio::test]
async fn api_role_listens_for_events_but_prunes_none() -> eyre::Result<()> {
    setup_tracing();

    let service = TestService::spawn(api_service()).await?;

    let tasks: Vec<_> = service
        .service
        .task_runner()
        .task_health()
        .into_iter()
        .map(|(label, _)| label)
        .collect();
    assert_eq!(tasks, ["listen_tx_events"]);

    Ok(())
}

#[tokio::test]
async fn poll_gets_events_while_the_listener_is_down() -> eyre::Result<()> {
    setup_tracing();

    let service = TestService::spawn(api_service()).await?;
    let client = service.v2_client(&service.api_key);
    send_tx_v2(&client, send_tx_request("tx-1")).await?;
    send_tx_v2(&client, send_tx_request("tx-2")).await?;
    let listener_pid = await_listener(&service.db, None).await?;

    let (events, stored_at) =
        tokio::join!(get_events(&client, None, None, Some(10)), async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            sqlx::query("SELECT pg_terminate_backend($1)")
                .bind(listener_pid)
                .execute(&service.db.pool)
                .await?;
            broadcast(&service.db, "tx-1", 1).await?;

            Ok::<_, eyre::Report>(Instant::now())
        });
    let woken_after = stored_at?.elapsed();

    assert_eq!(tx_hashes(&events?), [B256::with_last_byte(1)]);
    assert!(
        woken_after < Duration::from_millis(1_500),
        "Woken {woken_after:?} after the event was stored"
    );

    // The listener comes back, and wakes polls again
    await_listener(&service.db, Some(listener_pid)).await?;
    let cursor = get_events(&client, None, None, None).await?.next_cursor;
    let (events, stored_at) = tokio::join!(
        get_events(&client, Some(cursor), None, Some(10)),
        async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            broadcast(&service.db, "tx-2", 2).await?;

            Ok::<_, eyre::Report>(Instant::now())
        }
    );
    let woken_after = stored_at?.elapsed();

    assert_eq!(tx_hashes(&events?), [B256::with_last_byte(2)]);
    assert!(
        woken_after < Duration::from_millis(400),
        "Woken {woken_after:?} after the event was stored"
    );

    Ok(())
}

/// A service that serves the API and runs no background tasks, so that
/// events are only stored by the test
fn api_service() -> ServiceBuilder {
    ServiceBuilder::default().role(ServiceRole::Api)
}

fn send_tx_request(tx_id: &str) -> SendTxRequestV2 {
    SendTxRequestV2 {
        tx_id: Some(tx_id.to_string()),
        ..SendTxRequestV2::new(
            ARBITRARY_ADDRESS.into(),
            U256::ZERO.into(),
            U256::from(21_000).into(),
        )
    }
}

/// Stores the broadcast of `tx_id` with hash `0x..{hash_byte}`, which stores
/// its first event
async fn broadcast(
    db: &Database,
    tx_id: &str,
    hash_byte: u8,
) -> eyre::Result<()> {
    let fee = U256::from(1);
    db.insert_tx_broadcast(
        tx_id,
        B256::with_last_byte(hash_byte),
        &[hash_byte],
        fee,
        fee,
    )
    .await
}

async fn escalate(
    db: &Database,
    tx_id: &str,
    hash_byte: u8,
) -> eyre::Result<()> {
    let fee = U256::from(2);
    let escalated = db
        .escalate_tx(
            tx_id,
            0,
            B256::with_last_byte(hash_byte),
            &[hash_byte],
            fee,
            fee,
        )
        .await?;
    eyre::ensure!(escalated, "{tx_id} wasn't escalated");

    Ok(())
}

async fn get_events(
    client: &Configuration,
    after: Option<String>,
    limit: Option<i32>,
    wait: Option<i32>,
) -> Result<GetTxEventsResponse, Error<GetEventsV2Error>> {
    tx_sitter_client::apis::relayer_v2_api::get_events_v2(
        client,
        GetEventsV2Params { after, limit, wait },
    )
    .await
}

fn tx_hashes(page: &GetTxEventsResponse) -> Vec<B256> {
    page.events
        .iter()
        .filter_map(|event| event.tx_hash.as_ref())
        .map(|tx_hash| tx_hash.0)
        .collect()
}

/// Returns the backend pid of the service's listener for tx events once it
/// listens, on another connection than `replaced` if given
async fn await_listener(
    db: &Database,
    replaced: Option<i32>,
) -> eyre::Result<i32> {
    let deadline = Instant::now() + Duration::from_secs(20);

    while Instant::now() < deadline {
        let listener_pids: Vec<(i32,)> = sqlx::query_as(
            r#"SELECT pid FROM pg_stat_activity WHERE query = 'LISTEN "tx_events"'"#,
        )
        .fetch_all(&db.pool)
        .await?;
        if let Some(&(pid,)) =
            listener_pids.iter().find(|(pid,)| Some(*pid) != replaced)
        {
            return Ok(pid);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    eyre::bail!("No listener for tx events")
}

/// Sends a transfer of `value` with [`METADATA`] as its exact text
async fn send_raw_tx_v2(
    base_url: &str,
    api_key: &str,
    tx_id: &str,
    value: U256,
) -> eyre::Result<()> {
    reqwest::Client::new()
        .post(format!("{base_url}/2/tx"))
        .bearer_auth(api_key)
        .header("content-type", "application/json")
        .body(format!(
            r#"{{"to":"{ARBITRARY_ADDRESS}","value":"{value}","gasLimit":"21000","txId":"{tx_id}","metadata":{METADATA}}}"#
        ))
        .send()
        .await?
        .error_for_status()?;

    Ok(())
}

type RawEvent = HashMap<String, Box<RawValue>>;

/// Long-polls the feed after `after` until it returned events with each of
/// `statuses` in turn, and returns the events and the cursor after them
async fn await_events(
    base_url: &str,
    api_key: &str,
    mut after: Option<String>,
    statuses: &[TxStatus],
) -> eyre::Result<(Vec<RawEvent>, Option<String>)> {
    let http = reqwest::Client::new();
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut events = vec![];
    let mut awaited = statuses.iter().peekable();

    while awaited.peek().is_some() {
        eyre::ensure!(
            Instant::now() < deadline,
            "Still awaiting {:?} after {events:?}",
            awaited.collect::<Vec<_>>()
        );

        // Cursors are URL safe
        let after_param = match &after {
            Some(after) => format!("&after={after}"),
            None => String::new(),
        };
        let page: HashMap<String, Box<RawValue>> = http
            .get(format!("{base_url}/2/events?wait=10{after_param}"))
            .bearer_auth(api_key)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let page_events: Vec<RawEvent> =
            serde_json::from_str(page["events"].get())?;
        let next_cursor: String =
            serde_json::from_str(page["nextCursor"].get())?;

        for event in page_events {
            if awaited.peek() == Some(&&status(&event)) {
                awaited.next();
            }
            events.push(event);
        }
        after = Some(next_cursor);
    }

    Ok((events, after))
}

fn status(event: &RawEvent) -> TxStatus {
    let status: Value = serde_json::from_str(event["status"].get())
        .expect("An event's status is JSON");

    serde_json::from_value(status).expect("An event has a known status")
}
