use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Limits a repeated log to one per key, such as a transaction or relayer id,
/// per [`Self::PERIOD`]. A metric should count every occurrence.
pub struct RateLimitedLogs {
    last_logged: Mutex<BTreeMap<String, Instant>>,
}

impl RateLimitedLogs {
    pub const PERIOD: Duration = Duration::from_secs(10 * 60);

    pub const fn new() -> Self {
        Self {
            last_logged: Mutex::new(BTreeMap::new()),
        }
    }

    /// Returns `true`, and records `now`, unless `key` was logged less than
    /// [`Self::PERIOD`] before `now`.
    pub fn should_log(&self, key: &str, now: Instant) -> bool {
        let mut last_logged = self
            .last_logged
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        last_logged.retain(|_, logged_at| now - *logged_at < Self::PERIOD);

        if last_logged.contains_key(key) {
            return false;
        }

        last_logged.insert(key.to_string(), now);
        true
    }
}

impl Default for RateLimitedLogs {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_each_key_once_per_period() {
        let logs = RateLimitedLogs::new();
        let start = Instant::now();
        let half_period = start + RateLimitedLogs::PERIOD / 2;

        assert!(logs.should_log("tx_1", start));
        assert!(!logs.should_log("tx_1", half_period));
        assert!(logs.should_log("tx_2", half_period));
        assert!(logs.should_log("tx_1", start + RateLimitedLogs::PERIOD));
        assert!(!logs.should_log("tx_2", start + RateLimitedLogs::PERIOD));
    }
}
