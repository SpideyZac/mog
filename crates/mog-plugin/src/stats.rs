//! How fast a plugin answers: request counts, latency, timeouts and slow requests.

use std::{
    collections::VecDeque,
    mem,
    time::{Duration, Instant},
};

/// How many recent request times are kept for percentiles.
const RECENT: usize = 200;

/// How many timeouts are remembered.
const TIMEOUTS: usize = 32;

/// How many slow requests wait to be reported.
const SLOW_KEPT: usize = 16;

/// How long a request may take before it counts as slow. Commands are left out, since a
/// command doing real work is expected to take a while.
pub const SLOW: Duration = Duration::from_secs(1);

/// How a request ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The plugin answered.
    Answered,
    /// The plugin answered with an error, or stopped.
    Failed,
    /// The plugin did not answer in time.
    TimedOut,
}

/// Numbers about the requests mog sent one plugin.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    /// Requests sent.
    pub requests: u64,
    /// Requests that failed.
    pub failed: u64,
    /// Requests that timed out.
    pub timeouts: u64,
    /// Time spent waiting on all of them.
    pub total: Duration,
    /// The longest wait.
    pub slowest: Duration,
    /// Recent waits, oldest first.
    recent: VecDeque<Duration>,
    /// When recent timeouts happened, oldest first.
    timed_out: VecDeque<Instant>,
    /// Slow requests not reported yet, as `(method, wait)`.
    slow: Vec<(String, Duration)>,
}

impl Stats {
    /// Notes that the request `method` took `took` and ended with `outcome`.
    pub fn record(&mut self, method: &str, took: Duration, outcome: Outcome) {
        self.requests += 1;
        self.total += took;
        self.slowest = self.slowest.max(took);
        if self.recent.len() == RECENT {
            self.recent.pop_front();
        }
        self.recent.push_back(took);
        match outcome {
            Outcome::Answered => {}
            Outcome::Failed => self.failed += 1,
            Outcome::TimedOut => {
                self.timeouts += 1;
                if self.timed_out.len() == TIMEOUTS {
                    self.timed_out.pop_front();
                }
                self.timed_out.push_back(Instant::now());
            }
        }
        if (took >= SLOW || outcome == Outcome::TimedOut)
            && method != "command"
            && self.slow.len() < SLOW_KEPT
        {
            self.slow.push((method.to_owned(), took));
        }
    }

    /// Returns the mean wait, if anything was asked.
    pub fn mean(&self) -> Option<Duration> {
        let count = u32::try_from(self.requests)
            .ok()
            .filter(|count| *count > 0)?;
        Some(self.total / count)
    }

    /// Returns the wait that `fraction` of recent requests stayed under, like `0.95`.
    pub fn percentile(&self, fraction: f64) -> Option<Duration> {
        let mut sorted: Vec<Duration> = self.recent.iter().copied().collect();
        sorted.sort_unstable();
        let last = sorted.len().checked_sub(1)?;
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss,
            reason = "an index into a list of at most a few hundred items"
        )]
        let index = ((last as f64) * fraction.clamp(0.0, 1.0)).round() as usize;
        sorted.get(index).copied()
    }

    /// Returns how many requests timed out in the last `window`.
    pub fn timeouts_within(&self, window: Duration) -> usize {
        self.timed_out
            .iter()
            .filter(|at| at.elapsed() <= window)
            .count()
    }

    /// Returns the slow requests since the last call, as `(method, wait)`.
    pub fn take_slow(&mut self) -> Vec<(String, Duration)> {
        mem::take(&mut self.slow)
    }
}

#[cfg(test)]
/// Tests for request stats.
mod tests {
    use std::time::Duration;

    use super::{Outcome, Stats};

    /// Counts, means, percentiles, timeouts and slow requests add up.
    #[test]
    fn adds_up() {
        let mut stats = Stats::default();
        for ms in 1..=100 {
            stats.record(
                "provide/hover",
                Duration::from_millis(ms),
                Outcome::Answered,
            );
        }
        stats.record("command", Duration::from_secs(5), Outcome::Answered);
        stats.record("key", Duration::from_secs(1), Outcome::TimedOut);
        assert_eq!(stats.requests, 102);
        assert_eq!(stats.timeouts, 1);
        assert_eq!(stats.slowest, Duration::from_secs(5));
        assert_eq!(stats.percentile(0.5), Some(Duration::from_millis(52)));
        assert!(
            stats
                .mean()
                .is_some_and(|mean| mean > Duration::from_millis(50))
        );
        assert_eq!(stats.timeouts_within(Duration::from_secs(60)), 1);
        assert_eq!(
            stats.take_slow(),
            [("key".to_owned(), Duration::from_secs(1))]
        );
        assert!(stats.take_slow().is_empty());
        assert_eq!(Stats::default().percentile(0.95), None);
    }
}
