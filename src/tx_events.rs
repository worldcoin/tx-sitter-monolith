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

/// The least time a poll gives a read of the feed, also once its deadline is
/// near or past
const MIN_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the listener waits for a notification before it reports
/// progress, see [`TaskContext::wait`]
const LISTENER_PROGRESS_INTERVAL: Duration = Duration::from_secs(60);

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
    /// The ids of relayers whose events were stored
    wakeups: broadcast::Sender<Arc<str>>,
    waiter_slots: Arc<Semaphore>,
    shutdown: CancellationToken,
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

    /// Starts a request's poll of the feed of relayer `relayer_id`, which
    /// may wait up to `timeout` for events.
    ///
    /// A poll that may wait takes one of the notifier's `max_waiters` slots
    /// until it is dropped, and fails with [`TooManyWaiters`] if none is
    /// free. Start it before reading the feed, so that the slots also bound
    /// the reads, and no notification goes unseen between a read and
    /// [`LongPoll::wait`].
    pub fn long_poll(
        &self,
        relayer_id: &str,
        timeout: Duration,
    ) -> Result<LongPoll<'_>, TooManyWaiters> {
        let waiter = if timeout.is_zero() {
            None
        } else {
            let slot = self
                .waiter_slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| TooManyWaiters)?;

            Some(Waiter {
                wakeups: self.wakeups.subscribe(),
                _slot: slot,
            })
        };

        Ok(LongPoll {
            notifier: self,
            relayer_id: relayer_id.to_string(),
            deadline: Instant::now() + timeout,
            waiter,
        })
    }

    fn wake_relayer(&self, relayer_id: &str) {
        // Fails only while no request waits
        let _ = self.wakeups.send(relayer_id.into());
    }
}

/// A request's poll of a relayer's feed, see [`EventNotifier::long_poll`].
pub struct LongPoll<'a> {
    notifier: &'a EventNotifier,
    relayer_id: String,
    deadline: Instant,
    /// `None` for a poll that doesn't wait
    waiter: Option<Waiter>,
}

struct Waiter {
    wakeups: broadcast::Receiver<Arc<str>>,
    _slot: OwnedSemaphorePermit,
}

impl LongPoll<'_> {
    /// Returns when a read of the feed must finish: at the poll's deadline,
    /// or [`MIN_READ_TIMEOUT`] from now if that is later.
    pub fn read_deadline(&self) -> Instant {
        self.deadline.max(Instant::now() + MIN_READ_TIMEOUT)
    }

    /// Waits until events of the relayer may have been stored, and returns
    /// `true` once the feed is worth reading again.
    ///
    /// That is when a notification for the relayer wakes the poll, or after
    /// a second at most, in case a notification got lost. Returns `false`
    /// without waiting once the deadline passed or shutdown was requested.
    pub async fn wait(&mut self) -> bool {
        let now = Instant::now();
        let shutdown = &self.notifier.shutdown;
        let Some(waiter) = self.waiter.as_mut() else {
            return false;
        };

        if now >= self.deadline || shutdown.is_cancelled() {
            return false;
        }

        let recheck_at = self.deadline.min(now + RECHECK_INTERVAL);
        tokio::select! {
            () = shutdown.cancelled() => {}
            () = tokio::time::sleep_until(recheck_at) => {}
            () = next_wakeup(&mut waiter.wakeups, &self.relayer_id) => {}
        }

        true
    }
}

/// Returns once `wakeups` wakes relayer `relayer_id`, or skipped wakeups it
/// fell behind on
async fn next_wakeup(
    wakeups: &mut broadcast::Receiver<Arc<str>>,
    relayer_id: &str,
) {
    loop {
        match wakeups.recv().await {
            Ok(woken) if *woken != *relayer_id => continue,
            Ok(_) | Err(RecvError::Lagged(_)) => return,
            // The notifier, which owns the sender, outlives its polls
            Err(RecvError::Closed) => std::future::pending().await,
        }
    }
}

/// Listens for notifications of stored events and passes them on to
/// `notifier`.
///
/// A lost connection is replaced on the next read. A connection that fails
/// otherwise fails the task, which the task runner restarts with a new one.
/// Notifications sent while none listens are lost, and the waiting requests
/// find their events when they read the feed again, a second later at most.
pub async fn listen_for_tx_events(
    app: Arc<App>,
    ctx: TaskContext,
    notifier: Arc<EventNotifier>,
) -> eyre::Result<()> {
    let mut listener = app.db.listen_tx_events().await?;

    loop {
        // A read dropped halfway through a message would leave the rest of
        // it to the next read, so a wait that times out keeps the read going.
        // Only shutdown drops it, and the connection with it.
        let next_notification = listener.try_recv();
        tokio::pin!(next_notification);
        let notification = loop {
            match ctx
                .wait(LISTENER_PROGRESS_INTERVAL, &mut next_notification)
                .await
            {
                Wait::Ready(notification) => break notification,
                Wait::TimedOut => continue,
                Wait::ShuttingDown => return Ok(()),
            }
        };

        match notification.context("Listening for tx events")? {
            Some(notification) => notifier.wake_relayer(notification.payload()),
            None => tracing::warn!(
                "Lost the connection listening for tx events, reconnecting"
            ),
        }
    }
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
        let mut poll = notifier
            .long_poll("relayer", Duration::from_secs(30))
            .unwrap();
        let started = Instant::now();

        notifier.wake_relayer("other relayer");
        notifier.wake_relayer("relayer");

        assert!(poll.wait().await);
        assert_eq!(started.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn poll_rechecks_without_a_wakeup() {
        let notifier = EventNotifier::new(2, CancellationToken::new());
        let mut poll = notifier
            .long_poll("relayer", Duration::from_secs(30))
            .unwrap();
        let started = Instant::now();

        notifier.wake_relayer("other relayer");

        assert!(poll.wait().await);
        assert_eq!(started.elapsed(), RECHECK_INTERVAL);
    }

    #[tokio::test(start_paused = true)]
    async fn poll_ends_at_its_deadline_or_shutdown() {
        let shutdown = CancellationToken::new();
        let notifier = EventNotifier::new(2, shutdown.clone());
        let timeout = Duration::from_millis(2_500);
        let mut poll = notifier.long_poll("relayer", timeout).unwrap();
        let started = Instant::now();

        while poll.wait().await {}
        assert_eq!(started.elapsed(), timeout);

        let mut poll = notifier
            .long_poll("relayer", Duration::from_secs(30))
            .unwrap();
        shutdown.cancel();
        assert!(!poll.wait().await);

        let mut poll = notifier.long_poll("relayer", Duration::ZERO).unwrap();
        assert!(!poll.wait().await);
    }

    #[tokio::test(start_paused = true)]
    async fn reads_get_until_the_deadline_or_min_read_timeout() {
        let notifier = EventNotifier::new(2, CancellationToken::new());
        let started = Instant::now();

        let poll = notifier
            .long_poll("relayer", Duration::from_secs(30))
            .unwrap();
        assert_eq!(poll.read_deadline(), started + Duration::from_secs(30));

        tokio::time::advance(Duration::from_secs(29)).await;
        assert_eq!(poll.read_deadline(), Instant::now() + MIN_READ_TIMEOUT);

        let poll = notifier.long_poll("relayer", Duration::ZERO).unwrap();
        assert_eq!(poll.read_deadline(), Instant::now() + MIN_READ_TIMEOUT);
    }

    #[test]
    fn polls_beyond_max_waiters_are_refused() {
        let notifier = EventNotifier::new(1, CancellationToken::new());
        let timeout = Duration::from_secs(5);

        let waiting = notifier.long_poll("relayer", timeout).unwrap();
        assert!(matches!(
            notifier.long_poll("relayer", timeout),
            Err(TooManyWaiters)
        ));
        // A poll that doesn't wait needs no slot
        assert!(notifier.long_poll("relayer", Duration::ZERO).is_ok());

        drop(waiting);
        assert!(notifier.long_poll("relayer", timeout).is_ok());
    }
}
