//! Bounded retry with full-jitter backoff and a small circuit breaker for internal calls.
//!
//! Only idempotent operations may be retried. Callers decide which errors are transient.

use std::{
    future::Future,
    sync::Mutex,
    time::{Duration, Instant},
};

use rand::Rng;

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// Total attempts including the first one.
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self { max_attempts: 3, base_delay: Duration::from_millis(50), max_delay: Duration::from_millis(500) }
    }
}

impl RetryPolicy {
    /// Full jitter: uniform in `[0, min(max, base * 2^attempt)]`.
    fn delay(&self, attempt: u32) -> Duration {
        let cap = self.base_delay.saturating_mul(2_u32.saturating_pow(attempt)).min(self.max_delay);
        let millis = u64::try_from(cap.as_millis()).unwrap_or(u64::MAX);
        Duration::from_millis(rand::thread_rng().gen_range(0..=millis))
    }
}

/// Run `op` until it succeeds, fails with a non-transient error, or attempts run out.
pub async fn retry<T, E, F, Fut>(policy: RetryPolicy, is_transient: impl Fn(&E) -> bool, mut op: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let mut attempt = 0;
    loop {
        match op().await {
            Ok(value) => return Ok(value),
            Err(err) => {
                attempt += 1;
                if attempt >= policy.max_attempts || !is_transient(&err) {
                    return Err(err);
                }
                tokio::time::sleep(policy.delay(attempt)).await;
            }
        }
    }
}

#[derive(Debug)]
enum State {
    Closed {
        consecutive_failures: u32,
    },
    Open {
        until: Instant,
    },
    /// One trial call is in flight.
    HalfOpen,
}

/// Opens after `failure_threshold` consecutive failures, rejects calls for `open_for`,
/// then lets a single trial call through.
#[derive(Debug)]
pub struct CircuitBreaker {
    state: Mutex<State>,
    failure_threshold: u32,
    open_for: Duration,
}

#[derive(Debug, PartialEq, Eq)]
pub struct CircuitOpen;

impl CircuitBreaker {
    pub fn new(failure_threshold: u32, open_for: Duration) -> Self {
        Self { state: Mutex::new(State::Closed { consecutive_failures: 0 }), failure_threshold, open_for }
    }

    /// Ask permission to make a call. Report the outcome with [`Self::record`].
    pub fn try_acquire(&self) -> Result<(), CircuitOpen> {
        let Ok(mut state) = self.state.lock() else {
            return Ok(());
        };
        match *state {
            State::Closed { .. } => Ok(()),
            State::Open { until } if Instant::now() >= until => {
                *state = State::HalfOpen;
                Ok(())
            }
            State::Open { .. } | State::HalfOpen => Err(CircuitOpen),
        }
    }

    pub fn record(&self, success: bool) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        *state = match (&*state, success) {
            (_, true) => State::Closed { consecutive_failures: 0 },
            (State::Closed { consecutive_failures }, false) if consecutive_failures + 1 < self.failure_threshold => {
                State::Closed { consecutive_failures: consecutive_failures + 1 }
            }
            (_, false) => State::Open { until: Instant::now() + self.open_for },
        };
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new(5, Duration::from_secs(10))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    fn fast() -> RetryPolicy {
        RetryPolicy { max_attempts: 3, base_delay: Duration::from_millis(1), max_delay: Duration::from_millis(2) }
    }

    #[tokio::test]
    async fn retries_transient_errors_up_to_the_bound() {
        let calls = AtomicU32::new(0);
        let result: Result<(), &str> = retry(
            fast(),
            |_| true,
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Err("down") }
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn does_not_retry_permanent_errors() {
        let calls = AtomicU32::new(0);
        let result: Result<(), &str> = retry(
            fast(),
            |_| false,
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Err("bad request") }
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn succeeds_after_a_transient_failure() {
        let calls = AtomicU32::new(0);
        let result: Result<u32, &str> = retry(
            fast(),
            |_| true,
            || {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                async move {
                    if n == 0 {
                        Err("blip")
                    } else {
                        Ok(n)
                    }
                }
            },
        )
        .await;
        assert_eq!(result, Ok(1));
    }

    #[test]
    fn breaker_opens_then_allows_one_trial() {
        let breaker = CircuitBreaker::new(2, Duration::from_millis(20));
        assert!(breaker.try_acquire().is_ok());
        breaker.record(false);
        assert!(breaker.try_acquire().is_ok());
        breaker.record(false);
        assert_eq!(breaker.try_acquire(), Err(CircuitOpen));
        std::thread::sleep(Duration::from_millis(30));
        assert!(breaker.try_acquire().is_ok(), "half-open lets one call through");
        assert_eq!(breaker.try_acquire(), Err(CircuitOpen), "but only one");
        breaker.record(true);
        assert!(breaker.try_acquire().is_ok());
    }

    #[test]
    fn failed_trial_reopens() {
        let breaker = CircuitBreaker::new(1, Duration::from_millis(10));
        breaker.record(false);
        std::thread::sleep(Duration::from_millis(15));
        assert!(breaker.try_acquire().is_ok());
        breaker.record(false);
        assert_eq!(breaker.try_acquire(), Err(CircuitOpen));
    }
}
