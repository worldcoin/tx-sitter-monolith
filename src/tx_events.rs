//! The feed of transaction status changes that `GET /2/events` serves.
//!
//! Storing an event notifies its relayer's channel. Every process that serves
//! the API listens with one connection, see [`listen_for_tx_events`], and an
//! [`EventNotifier`] passes the notifications on to the requests waiting for
//! events.

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use base64::prelude::BASE64_URL_SAFE_NO_PAD;
use base64::Engine;
use eyre::Context;
use sqlx::FromRow;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::app::App;
use crate::db::wrappers::xid8::Xid8;
use crate::task_runner::{TaskContext, Wait};

/// How often a waiting request reads the feed without a notification, which
/// it may have missed
const RECHECK_INTERVAL: Duration = Duration::from_secs(1);

/// How long the listener goes without a notification before it replaces its
/// connection, which may have died without an error
const LISTENER_QUIET_TIMEOUT: Duration = Duration::from_secs(60);

/// How many wakeups a waiting request may fall behind by before it reads the
/// feed again regardless of their relayers
const WAKEUP_BUFFER: usize = 256;

/// The position of an event in the feed.
///
/// The feed orders events by the database transaction that stored them, then
/// by their id, see
/// [`Database::read_tx_events`](crate::db::Database::read_tx_events). The
/// text form is opaque to clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, FromRow)]
pub struct EventCursor {
    pub tx_xid: Xid8,
    pub id: i64,
}

impl EventCursor {
    /// The position before every event
    pub const START: Self = Self {
        tx_xid: Xid8(0),
        id: 0,
    };

    /// The length of the text form: 16 bytes in unpadded base64
    const TEXT_LEN: usize = 22;
}

impl fmt::Display for EventCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut bytes = [0; 16];
        bytes[..8].copy_from_slice(&self.tx_xid.0.to_be_bytes());
        bytes[8..].copy_from_slice(&self.id.to_be_bytes());

        f.write_str(&BASE64_URL_SAFE_NO_PAD.encode(bytes))
    }
}

impl FromStr for EventCursor {
    type Err = eyre::Report;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        eyre::ensure!(
            s.len() == Self::TEXT_LEN,
            "A cursor is {} characters long",
            Self::TEXT_LEN
        );

        let bytes: [u8; 16] = BASE64_URL_SAFE_NO_PAD
            .decode(s)?
            .try_into()
            .map_err(|_| eyre::eyre!("A cursor is 16 bytes"))?;
        let (tx_xid, id) = bytes.split_at(8);

        Ok(Self {
            tx_xid: Xid8(u64::from_be_bytes(tx_xid.try_into()?)),
            id: i64::from_be_bytes(id.try_into()?),
        })
    }
}

/// Passes notifications of stored events on to the requests waiting for
/// them, see [`EventNotifier::long_poll`].
///
/// It lets at most `max_waiters` requests wait at once, and wakes all of them
/// once shutdown is requested.
pub struct EventNotifier {
    wakeups: broadcast::Sender<Wakeup>,
    waiter_slots: Arc<Semaphore>,
    shutdown: CancellationToken,
}

#[derive(Debug, Clone)]
enum Wakeup {
    /// Events of the relayer were stored
    Relayer(Arc<str>),
    /// Events may have been stored without a notification
    All,
}

/// A request can't wait for events, because the most requests the process
/// lets wait already do
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooManyWaiters;

impl EventNotifier {
    pub fn new(max_waiters: usize, shutdown: CancellationToken) -> Self {
        let (wakeups, _) = broadcast::channel(WAKEUP_BUFFER);

        Self {
            wakeups,
            waiter_slots: Arc::new(Semaphore::new(max_waiters)),
            shutdown,
        }
    }

    /// Starts a wait of up to `timeout` for events of relayer `relayer_id`.
    ///
    /// Start it before reading the feed, so that no notification goes unseen
    /// between the read and [`LongPoll::wait`].
    pub fn long_poll(
        &self,
        relayer_id: &str,
        timeout: Duration,
    ) -> LongPoll<'_> {
        LongPoll {
            notifier: self,
            relayer_id: relayer_id.to_string(),
            deadline: Instant::now() + timeout,
            wakeups: (!timeout.is_zero()).then(|| self.wakeups.subscribe()),
            waiter_slot: None,
        }
    }

    fn wake_relayer(&self, relayer_id: &str) {
        // Fails only while no request waits
        let _ = self.wakeups.send(Wakeup::Relayer(relayer_id.into()));
    }

    fn wake_all(&self) {
        let _ = self.wakeups.send(Wakeup::All);
    }
}

/// A request's wait for events of a relayer, see
/// [`EventNotifier::long_poll`].
pub struct LongPoll<'a> {
    notifier: &'a EventNotifier,
    relayer_id: String,
    deadline: Instant,
    /// `None` for a poll that doesn't wait
    wakeups: Option<broadcast::Receiver<Wakeup>>,
    /// Taken on the first wait and kept until the poll is dropped
    waiter_slot: Option<OwnedSemaphorePermit>,
}

impl LongPoll<'_> {
    /// Waits until events of the relayer may have been stored, and returns
    /// `true` once the feed is worth reading again.
    ///
    /// That is when a notification for the relayer or a reconnected
    /// listener wakes the poll, or after a second at most, in case a
    /// notification got lost. Returns `false` without waiting once the
    /// deadline passed or shutdown was requested.
    ///
    /// The first wait takes one of the notifier's `max_waiters` slots, and
    /// fails with [`TooManyWaiters`] if none is free.
    pub async fn wait(&mut self) -> Result<bool, TooManyWaiters> {
        let now = Instant::now();
        let shutdown = &self.notifier.shutdown;
        let Some(wakeups) = self.wakeups.as_mut() else {
            return Ok(false);
        };

        if now >= self.deadline || shutdown.is_cancelled() {
            return Ok(false);
        }

        if self.waiter_slot.is_none() {
            let waiter_slot = self
                .notifier
                .waiter_slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| TooManyWaiters)?;
            self.waiter_slot = Some(waiter_slot);
        }

        let recheck_at = self.deadline.min(now + RECHECK_INTERVAL);
        tokio::select! {
            () = shutdown.cancelled() => {}
            () = tokio::time::sleep_until(recheck_at) => {}
            () = next_wakeup(wakeups, &self.relayer_id) => {}
        }

        Ok(true)
    }
}

/// Returns once `wakeups` wakes relayer `relayer_id`, or skipped wakeups it
/// fell behind on
async fn next_wakeup(
    wakeups: &mut broadcast::Receiver<Wakeup>,
    relayer_id: &str,
) {
    loop {
        match wakeups.recv().await {
            Ok(Wakeup::Relayer(woken)) if *woken != *relayer_id => continue,
            Ok(_) | Err(RecvError::Lagged(_)) => return,
            // The notifier, which owns the sender, outlives its polls
            Err(RecvError::Closed) => std::future::pending().await,
        }
    }
}

/// Listens for notifications of stored events and passes them on to
/// `notifier`.
///
/// Notifications sent while no connection listens are lost, so every new
/// connection wakes every waiting request to read the feed. A connection
/// that is lost, or that delivers no notification for a minute, is replaced:
/// one that died without an error would deliver none.
pub async fn listen_for_tx_events(
    app: Arc<App>,
    ctx: TaskContext,
    notifier: Arc<EventNotifier>,
) -> eyre::Result<()> {
    while !ctx.is_shutting_down() {
        let mut listener = app.db.listen_tx_events().await?;
        notifier.wake_all();

        loop {
            match ctx.wait(LISTENER_QUIET_TIMEOUT, listener.try_recv()).await {
                Wait::Ready(Ok(Some(notification))) => {
                    notifier.wake_relayer(notification.payload());
                }
                Wait::Ready(Ok(None)) => {
                    eyre::bail!("Lost the connection listening for tx events")
                }
                Wait::Ready(Err(err)) => {
                    return Err(err).context("Listening for tx events");
                }
                Wait::TimedOut => break,
                Wait::ShuttingDown => return Ok(()),
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test]
    fn cursor_text_round_trips() {
        let cursors = [
            EventCursor::START,
            EventCursor {
                tx_xid: Xid8(1_234_567),
                id: 89,
            },
            EventCursor {
                tx_xid: Xid8(u64::MAX),
                id: i64::MAX,
            },
        ];

        for cursor in cursors {
            let text = cursor.to_string();

            assert_eq!(text.len(), EventCursor::TEXT_LEN, "{text}");
            assert_eq!(text.parse::<EventCursor>().unwrap(), cursor);
        }
    }

    #[test_case("" ; "empty")]
    #[test_case("AAAAAAAS1ocAAAAAAAAAWQ=" ; "padded")]
    #[test_case("AAAAAAAS1ocAAAAAAAAAW" ; "short")]
    #[test_case("AAAAAAAS1ocAAAAAAAAAWQA" ; "long")]
    #[test_case("AAAAAAAS1ocAAAAAAAAA+Q" ; "not url safe")]
    #[test_case("AAAAAAAS1ocAAAAAAAAAW!" ; "not base64")]
    fn cursor_rejects_invalid_text(text: &str) {
        assert!(text.parse::<EventCursor>().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn poll_wakes_on_its_relayers_events() {
        let notifier = EventNotifier::new(2, CancellationToken::new());
        let mut poll = notifier.long_poll("relayer", Duration::from_secs(30));
        let started = Instant::now();

        notifier.wake_relayer("other relayer");
        notifier.wake_relayer("relayer");

        assert_eq!(poll.wait().await, Ok(true));
        assert_eq!(started.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn poll_rechecks_without_a_wakeup() {
        let notifier = EventNotifier::new(2, CancellationToken::new());
        let mut poll = notifier.long_poll("relayer", Duration::from_secs(30));
        let started = Instant::now();

        notifier.wake_relayer("other relayer");

        assert_eq!(poll.wait().await, Ok(true));
        assert_eq!(started.elapsed(), RECHECK_INTERVAL);
    }

    #[tokio::test(start_paused = true)]
    async fn poll_ends_at_its_deadline_or_shutdown() {
        let shutdown = CancellationToken::new();
        let notifier = EventNotifier::new(2, shutdown.clone());
        let timeout = Duration::from_millis(2_500);
        let mut poll = notifier.long_poll("relayer", timeout);
        let started = Instant::now();

        while poll.wait().await == Ok(true) {}
        assert_eq!(started.elapsed(), timeout);

        let mut poll = notifier.long_poll("relayer", Duration::from_secs(30));
        shutdown.cancel();
        assert_eq!(poll.wait().await, Ok(false));

        let mut poll = notifier.long_poll("relayer", Duration::ZERO);
        assert_eq!(poll.wait().await, Ok(false));
    }

    #[tokio::test(start_paused = true)]
    async fn polls_beyond_max_waiters_are_refused() {
        let notifier = EventNotifier::new(1, CancellationToken::new());
        let mut waiting = notifier.long_poll("relayer", Duration::from_secs(5));
        let mut refused = notifier.long_poll("relayer", Duration::from_secs(5));

        assert_eq!(waiting.wait().await, Ok(true));
        assert_eq!(refused.wait().await, Err(TooManyWaiters));

        // A poll keeps its slot between waits, and frees it once dropped
        assert_eq!(waiting.wait().await, Ok(true));
        drop(waiting);
        assert_eq!(refused.wait().await, Ok(true));
    }
}
