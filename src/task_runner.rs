use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::Future;
use tokio::task::JoinHandle;

const FAILURE_MONITORING_PERIOD: Duration = Duration::from_secs(60);

pub struct TaskRunner<T> {
    app: Arc<T>,
}

impl<T> TaskRunner<T> {
    pub fn new(app: Arc<T>) -> Self {
        Self { app }
    }
}

impl<T> TaskRunner<T>
where
    T: Send + Sync + 'static,
{
    pub fn add_task<S, C, F>(&self, label: S, task: C) -> JoinHandle<()>
    where
        S: ToString,
        C: Fn(Arc<T>) -> F + Send + Sync + 'static,
        F: Future<Output = eyre::Result<()>> + Send + 'static,
    {
        let app = self.app.clone();
        let label = label.to_string();

        tokio::spawn(async move {
            let mut failures = vec![];

            loop {
                tracing::info!(task_label = label, "Running task");

                let result = task(app.clone()).await;

                if let Err(err) = result {
                    tracing::error!(task_label = label, error = ?err, "Task failed");

                    failures.push(Instant::now());
                    let backoff = determine_backoff(&failures);

                    tokio::time::sleep(backoff).await;

                    prune_failures(&mut failures);
                } else {
                    tracing::info!(task_label = label, "Task finished");
                    break;
                }
            }
        })
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

fn prune_failures(failures: &mut Vec<Instant>) {
    failures.retain(|instant| instant.elapsed() < FAILURE_MONITORING_PERIOD);
}
