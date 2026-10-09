//! The wall-clock seam. Production code reads "now" only through [`Clock`].

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Epoch-millisecond wall clock (JS `Date.now()`).
pub trait Clock: Send + Sync + 'static {
    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> i64;

    /// The same instant as a [`jiff::Timestamp`], saturating at jiff's range.
    fn now(&self) -> jiff::Timestamp {
        timestamp_from_ms(self.now_ms())
    }
}

/// Shared clock handle carried by `AppContext` and the store.
pub type SharedClock = Arc<dyn Clock>;

/// Converts epoch milliseconds to a timestamp, saturating outside jiff's range.
pub fn timestamp_from_ms(ms: i64) -> jiff::Timestamp {
    jiff::Timestamp::from_millisecond(ms).unwrap_or(if ms < 0 {
        jiff::Timestamp::MIN
    } else {
        jiff::Timestamp::MAX
    })
}

/// The operating-system clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(elapsed) => i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
            Err(before) => i64::try_from(before.duration().as_millis())
                .map(|ms| -ms)
                .unwrap_or(i64::MIN),
        }
    }
}

/// A settable clock that advances with tokio's (possibly paused) time.
///
/// `now_ms` is the base epoch plus the tokio time elapsed since the base was
/// set, so `#[tokio::test(start_paused = true)]` plus `tokio::time::advance`
/// moves it deterministically.
#[derive(Debug)]
pub struct TestClock {
    state: Mutex<TestClockState>,
}

#[derive(Debug, Clone, Copy)]
struct TestClockState {
    base_ms: i64,
    anchor: tokio::time::Instant,
}

impl TestClock {
    /// A clock reading `epoch_ms` now.
    pub fn new(epoch_ms: i64) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(TestClockState {
                base_ms: epoch_ms,
                anchor: tokio::time::Instant::now(),
            }),
        })
    }

    /// Jumps the clock to `ms`; it keeps advancing with tokio time afterwards.
    pub fn set(&self, ms: i64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *state = TestClockState {
            base_ms: ms,
            anchor: tokio::time::Instant::now(),
        };
    }
}

impl Clock for TestClock {
    fn now_ms(&self) -> i64 {
        let state = *self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let elapsed = i64::try_from(state.anchor.elapsed().as_millis()).unwrap_or(i64::MAX);
        state.base_ms.saturating_add(elapsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn test_clock_follows_paused_time() {
        let clock = TestClock::new(1_000);
        assert_eq!(clock.now_ms(), 1_000);
        tokio::time::advance(std::time::Duration::from_millis(250)).await;
        assert_eq!(clock.now_ms(), 1_250);
        clock.set(5_000);
        assert_eq!(clock.now_ms(), 5_000);
        assert_eq!(clock.now().as_millisecond(), 5_000);
    }

    #[test]
    fn system_clock_is_after_2020() {
        assert!(SystemClock.now_ms() > 1_577_836_800_000);
    }
}
