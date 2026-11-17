use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

const FAILURE_MONITORING_PERIOD: Duration = Duration::from_secs(60);
const WATCHDOG_INTERVAL: Duration = Duration::from_secs(10);

/// Runs labelled background tasks, restarting them when they fail, and keeps
/// track of whether they are still making progress.
///
/// Every task gets a [`TaskContext`]. A task reports progress through it, and
/// stops at the next unit of work once shutdown is requested. A task that goes
/// longer than the stall timeout without reporting progress is stalled, see
/// [`TaskRunner::task_health`].
pub struct TaskRunner<T> {
    app: Arc<T>,
    shutdown: CancellationToken,
    stall_timeout: Duration,
    tasks: Mutex<HashMap<String, RunningTask>>,
}

struct RunningTask {
    progress: Arc<Progress>,
    handle: JoinHandle<()>,
}

/// Whether a task is still doing its job, see [`TaskRunner::task_health`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskHealth {
    /// The task reported progress within the stall timeout
    Progressing,
    /// The task went longer than the stall timeout without reporting progress
    Stalled,
    /// The task returned or panicked before shutdown was requested
    Stopped,
}

impl<T> TaskRunner<T>
where
    T: Send + Sync + 'static,
{
    pub fn new(
        app: Arc<T>,
        shutdown: CancellationToken,
        stall_timeout: Duration,
    ) -> Self {
        Self {
            app,
            shutdown,
            stall_timeout,
            tasks: Mutex::new(HashMap::new()),
        }
    }

    /// Spawns `task` under `label`, restarting it with a backoff whenever it
    /// returns an error.
    ///
    /// The label identifies the task in logs and metrics. If a task with the
    /// same label was already added, or shutdown was requested, this does
    /// nothing.
    pub fn add_task<S, C, F>(&self, label: S, task: C)
    where
        S: ToString,
        C: Fn(Arc<T>, TaskContext) -> F + Send + Sync + 'static,
        F: Future<Output = eyre::Result<()>> + Send + 'static,
    {
        let label = label.to_string();
        let mut tasks = self.lock_tasks();

        if tasks.contains_key(&label) || self.shutdown.is_cancelled() {
            return;
        }

        let ctx = TaskContext::new(self.shutdown.clone());
        let progress = ctx.progress.clone();
        let handle =
            tokio::spawn(run_task(label.clone(), self.app.clone(), ctx, task));

        tasks.insert(label, RunningTask { progress, handle });
    }

    /// Spawns a task that logs an error and sets the `task_stalled` gauge
    /// whenever a task stops making progress.
    pub fn add_watchdog(self: &Arc<Self>) {
        let runner = self.clone();

        self.add_task("watchdog", move |_, ctx| {
            watch_tasks(runner.clone(), ctx)
        });
    }

    /// Returns the health of every task, sorted by label.
    pub fn task_health(&self) -> Vec<(String, TaskHealth)> {
        let now = Instant::now();
        let is_shutting_down = self.shutdown.is_cancelled();

        let mut task_health: Vec<(String, TaskHealth)> = self
            .lock_tasks()
            .iter()
            .map(|(label, task)| {
                let deadline =
                    task.progress.reported_until() + self.stall_timeout;

                let health = if task.handle.is_finished() && !is_shutting_down {
                    TaskHealth::Stopped
                } else if now > deadline {
                    TaskHealth::Stalled
                } else {
                    TaskHealth::Progressing
                };

                (label.clone(), health)
            })
            .collect();
        task_health.sort_by(|(a, _), (b, _)| a.cmp(b));

        task_health
    }

    /// Waits for every task to finish, up to `deadline`.
    ///
    /// Tasks only finish once shutdown is requested, so this is meant to be
    /// called after the shutdown token is cancelled. Tasks still running at
    /// the deadline are aborted, and their labels are returned.
    pub async fn join(&self, deadline: Instant) -> Vec<String> {
        let tasks = std::mem::take(&mut *self.lock_tasks());

        let mut unfinished = vec![];
        for (label, task) in tasks {
            let mut handle = task.handle;

            match tokio::time::timeout_at(deadline, &mut handle).await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    tracing::error!(task_label = label, error = ?err, "Task panicked");
                }
                Err(_) => {
                    handle.abort();
                    unfinished.push(label);
                }
            }
        }

        unfinished
    }

    fn lock_tasks(&self) -> MutexGuard<'_, HashMap<String, RunningTask>> {
        self.tasks.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

async fn run_task<T, C, F>(
    label: String,
    app: Arc<T>,
    ctx: TaskContext,
    task: C,
) where
    C: Fn(Arc<T>, TaskContext) -> F,
    F: Future<Output = eyre::Result<()>>,
{
    let mut failures = vec![];

    while !ctx.is_shutting_down() {
        tracing::info!(task_label = label, "Running task");
        ctx.report_progress();

        let Err(err) = task(app.clone(), ctx.clone()).await else {
            if !ctx.is_shutting_down() {
                tracing::error!(
                    task_label = label,
                    "Task stopped before shutdown was requested"
                );
            }
            break;
        };

        tracing::error!(task_label = label, error = ?err, "Task failed");
        metrics::counter!("task_failures", &[("task", label.clone())])
            .increment(1);

        failures.push(Instant::now());
        prune_failures(&mut failures);

        ctx.idle(with_jitter(determine_backoff(&failures))).await;
    }

    tracing::info!(task_label = label, "Task finished");
}

async fn watch_tasks<T>(
    runner: Arc<TaskRunner<T>>,
    ctx: TaskContext,
) -> eyre::Result<()>
where
    T: Send + Sync + 'static,
{
    let mut unhealthy_before = HashSet::new();

    while !ctx.is_shutting_down() {
        let mut unhealthy = HashSet::new();

        for (label, health) in runner.task_health() {
            let is_progressing = health == TaskHealth::Progressing;

            metrics::gauge!("task_stalled", &[("task", label.clone())])
                .set(if is_progressing { 0.0 } else { 1.0 });

            if is_progressing {
                continue;
            }

            if !unhealthy_before.contains(&label) {
                tracing::error!(
                    task_label = label,
                    ?health,
                    stall_timeout = ?runner.stall_timeout,
                    "Task is not making progress"
                );
            }

            unhealthy.insert(label);
        }

        unhealthy_before = unhealthy;
        ctx.idle(WATCHDOG_INTERVAL).await;
    }

    Ok(())
}

/// A task's handle on the shutdown signal and on its progress deadline.
#[derive(Clone)]
pub struct TaskContext {
    shutdown: CancellationToken,
    progress: Arc<Progress>,
}

/// The outcome of [`TaskContext::wait`].
#[derive(Debug)]
pub enum Wait<T> {
    Ready(T),
    TimedOut,
    ShuttingDown,
}

impl TaskContext {
    /// Creates the context of a task that stops once `shutdown` is cancelled.
    pub(crate) fn new(shutdown: CancellationToken) -> Self {
        Self {
            shutdown,
            progress: Arc::new(Progress::new()),
        }
    }

    /// Returns `true` once shutdown is requested.
    ///
    /// Tasks check this between units of work and return `Ok(())` when it is
    /// set, so that no unit of work is interrupted halfway.
    pub fn is_shutting_down(&self) -> bool {
        self.shutdown.is_cancelled()
    }

    /// Records that the task made progress.
    ///
    /// The task stalls if it doesn't report progress again within the stall
    /// timeout.
    pub fn report_progress(&self) {
        self.progress.report(Duration::ZERO);
    }

    /// Sleeps for `duration`, returning early once shutdown is requested.
    ///
    /// The task counts as progressing while it sleeps.
    pub async fn idle(&self, duration: Duration) {
        self.wait(duration, std::future::pending::<()>()).await;
    }

    /// Waits up to `timeout` for `future`, returning early once shutdown is
    /// requested.
    ///
    /// The task counts as progressing while it waits, and has the regular
    /// stall timeout again once this returns. Dropping `future` on timeout or
    /// shutdown must be safe, so only wait for futures that don't do any work
    /// that has to complete, such as the next item of a stream.
    pub async fn wait<F: Future>(
        &self,
        timeout: Duration,
        future: F,
    ) -> Wait<F::Output> {
        self.progress.report(timeout);

        let outcome = tokio::select! {
            biased;
            _ = self.shutdown.cancelled() => Wait::ShuttingDown,
            result = tokio::time::timeout(timeout, future) => match result {
                Ok(output) => Wait::Ready(output),
                Err(_) => Wait::TimedOut,
            },
        };

        self.report_progress();

        outcome
    }
}

/// The instant up to which a task is known to be making progress.
struct Progress {
    reported_until: Mutex<Instant>,
}

impl Progress {
    fn new() -> Self {
        Self {
            reported_until: Mutex::new(Instant::now()),
        }
    }

    /// Records that the task made progress and may go quiet for
    /// `quiet_period` from now.
    fn report(&self, quiet_period: Duration) {
        *self.lock() = Instant::now() + quiet_period;
    }

    fn reported_until(&self) -> Instant {
        *self.lock()
    }

    fn lock(&self) -> MutexGuard<'_, Instant> {
        self.reported_until
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

fn determine_backoff(failures: &[Instant]) -> Duration {
    let mut base_backoff = Duration::from_secs(5);

    let num_failures_within_period = failures
        .iter()
        .filter(|&instant| instant.elapsed() < FAILURE_MONITORING_PERIOD)
        .count();

    if num_failures_within_period < 5 {
        // I second for each failure
        base_backoff += Duration::from_secs(num_failures_within_period as u64);
    }

    if num_failures_within_period > 5 {
        base_backoff += Duration::from_secs(10);
    }

    if num_failures_within_period > 10 {
        base_backoff += Duration::from_secs(30);
    }

    base_backoff
}

/// Adds up to 50% to `backoff` so that tasks failing together don't retry
/// together.
pub(crate) fn with_jitter(backoff: Duration) -> Duration {
    backoff + (backoff / 2).mul_f64(rand::random::<f64>())
}

fn prune_failures(failures: &mut Vec<Instant>) {
    failures.retain(|instant| instant.elapsed() < FAILURE_MONITORING_PERIOD);
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    const STALL_TIMEOUT: Duration = Duration::from_secs(60);

    fn runner() -> (Arc<TaskRunner<()>>, CancellationToken) {
        let shutdown = CancellationToken::new();
        let runner =
            TaskRunner::new(Arc::new(()), shutdown.clone(), STALL_TIMEOUT);

        (Arc::new(runner), shutdown)
    }

    async fn report_until_shutdown(ctx: TaskContext) -> eyre::Result<()> {
        while !ctx.is_shutting_down() {
            ctx.idle(Duration::from_secs(1)).await;
        }

        Ok(())
    }

    fn unhealthy_tasks(runner: &TaskRunner<()>) -> Vec<(String, TaskHealth)> {
        runner
            .task_health()
            .into_iter()
            .filter(|(_, health)| *health != TaskHealth::Progressing)
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn task_without_progress_stalls() {
        let (runner, _shutdown) = runner();
        runner.add_task("hung", |_, _| std::future::pending());
        runner.add_task("healthy", |_, ctx| report_until_shutdown(ctx));

        tokio::time::sleep(STALL_TIMEOUT / 2).await;
        assert!(unhealthy_tasks(&runner).is_empty());

        tokio::time::sleep(STALL_TIMEOUT).await;
        assert_eq!(
            unhealthy_tasks(&runner),
            vec![("hung".to_string(), TaskHealth::Stalled)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn task_hanging_after_a_completed_wait_stalls() {
        let (runner, _shutdown) = runner();
        runner.add_task("hung_after_wait", |_, ctx| async move {
            ctx.wait(STALL_TIMEOUT * 10, std::future::ready(())).await;
            std::future::pending().await
        });

        tokio::time::sleep(STALL_TIMEOUT + Duration::from_secs(1)).await;

        assert_eq!(
            unhealthy_tasks(&runner),
            vec![("hung_after_wait".to_string(), TaskHealth::Stalled)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn task_that_stops_before_shutdown_is_reported_at_once() {
        let (runner, _shutdown) = runner();
        runner.add_task("returned", |_, _| async { Ok(()) });
        runner.add_task("panicked", |_, _| async { panic!("task panic") });
        runner.add_task("healthy", |_, ctx| report_until_shutdown(ctx));

        tokio::time::sleep(Duration::from_secs(1)).await;

        assert_eq!(
            unhealthy_tasks(&runner),
            vec![
                ("panicked".to_string(), TaskHealth::Stopped),
                ("returned".to_string(), TaskHealth::Stopped),
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn idle_task_does_not_stall() {
        let (runner, _shutdown) = runner();
        runner.add_task("idle", |_, ctx| async move {
            ctx.idle(STALL_TIMEOUT * 10).await;
            Ok(())
        });

        tokio::time::sleep(STALL_TIMEOUT * 5).await;

        assert!(unhealthy_tasks(&runner).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn duplicate_label_is_ignored() {
        let (runner, shutdown) = runner();
        let starts = Arc::new(AtomicUsize::new(0));

        for _ in 0..2 {
            let starts = starts.clone();
            runner.add_task("task", move |_, ctx| {
                starts.fetch_add(1, Ordering::SeqCst);
                report_until_shutdown(ctx)
            });
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
        shutdown.cancel();
        runner.join(Instant::now() + Duration::from_secs(1)).await;

        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn failed_task_restarts_until_shutdown() {
        let (runner, shutdown) = runner();
        let starts = Arc::new(AtomicUsize::new(0));

        let task_starts = starts.clone();
        runner.add_task("failing", move |_, _| {
            task_starts.fetch_add(1, Ordering::SeqCst);
            async { eyre::bail!("failure") }
        });

        tokio::time::sleep(Duration::from_secs(30)).await;
        assert!(starts.load(Ordering::SeqCst) > 1);
        assert!(unhealthy_tasks(&runner).is_empty());

        shutdown.cancel();
        let unfinished =
            runner.join(Instant::now() + Duration::from_secs(1)).await;
        let starts_at_shutdown = starts.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_secs(30)).await;

        assert!(unfinished.is_empty());
        assert_eq!(starts.load(Ordering::SeqCst), starts_at_shutdown);
    }

    #[tokio::test(start_paused = true)]
    async fn join_reports_tasks_that_ignore_shutdown() {
        let (runner, shutdown) = runner();
        runner.add_task("stubborn", |_, _| std::future::pending());
        runner.add_task("cooperative", |_, ctx| report_until_shutdown(ctx));

        tokio::time::sleep(Duration::from_secs(1)).await;
        shutdown.cancel();
        let unfinished =
            runner.join(Instant::now() + Duration::from_secs(5)).await;

        assert_eq!(unfinished, vec!["stubborn".to_string()]);
    }

    #[tokio::test(start_paused = true)]
    async fn no_tasks_are_added_after_shutdown() {
        let (runner, shutdown) = runner();
        shutdown.cancel();

        runner.add_task("late", |_, _| std::future::pending());

        assert!(runner.join(Instant::now()).await.is_empty());
    }

    #[test]
    fn jitter_stays_within_half_of_backoff() {
        let backoff = Duration::from_secs(10);

        for _ in 0..100 {
            let jittered = with_jitter(backoff);
            assert!(jittered >= backoff);
            assert!(jittered <= backoff * 3 / 2);
        }
    }
}
