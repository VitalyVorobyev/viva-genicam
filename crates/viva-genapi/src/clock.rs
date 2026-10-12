//! The time a [`NodeMap`](crate::NodeMap) measures `<PollingTime>` against.
//!
//! A register with a `<PollingTime>` holds its cached value for that long and
//! then reads the device again (GenICam Standard v2.1.1 §2.8.3). The node map
//! asks a [`Clock`] for the time rather than `std::time::Instant` itself:
//! `Instant::now()` panics on `wasm32-unknown-unknown`, where Viva Studio
//! browses XML offline, and a test needs to move time by hand. A clock is
//! consulted only for a node that polls, so a document without
//! `<PollingTime>` never asks it.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// A monotonic time source.
pub trait Clock: fmt::Debug + Send + Sync {
    /// Time since an origin of the clock's choosing. Never decreases.
    fn now(&self) -> Duration;
}

/// The system's monotonic clock, `std::time::Instant`.
///
/// Not for `wasm32-unknown-unknown`, where `Instant::now()` panics; a node map
/// built there starts with a [`ManualClock`] instead.
#[derive(Debug, Clone, Copy)]
pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    /// A clock whose origin is now.
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

/// A clock that moves only when told to: for tests, and for a host without
/// a system clock, which advances it from its own (a browser's
/// `performance.now()`, say).
///
/// Shared through an `Arc`, so a test keeps a handle on the clock it gave the
/// node map.
#[derive(Debug, Default)]
pub struct ManualClock {
    nanos: AtomicU64,
}

impl ManualClock {
    /// A clock standing at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Move the clock forward by `by`.
    pub fn advance(&self, by: Duration) {
        let by = u64::try_from(by.as_nanos()).unwrap_or(u64::MAX);
        let mut current = self.nanos.load(Ordering::SeqCst);
        loop {
            let next = current.saturating_add(by);
            match self.nanos.compare_exchange_weak(
                current,
                next,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return,
                Err(actual) => current = actual,
            }
        }
    }

    /// Set the time to `to`; a time earlier than now is ignored, so the clock
    /// stays monotonic.
    pub fn set(&self, to: Duration) {
        let to = u64::try_from(to.as_nanos()).unwrap_or(u64::MAX);
        self.nanos.fetch_max(to, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Duration {
        Duration::from_nanos(self.nanos.load(Ordering::SeqCst))
    }
}

/// The clock a node map starts with: the system clock, or on
/// `wasm32-unknown-unknown`, which has none, a [`ManualClock`] that stands
/// still until the host replaces it — polled values then never expire.
pub(crate) fn default_clock() -> std::sync::Arc<dyn Clock> {
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    {
        std::sync::Arc::new(ManualClock::new())
    }
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    {
        std::sync::Arc::new(SystemClock::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manual_clock_moves_only_forward() {
        let clock = ManualClock::new();
        assert_eq!(clock.now(), Duration::ZERO);
        clock.advance(Duration::from_millis(500));
        assert_eq!(clock.now(), Duration::from_millis(500));
        clock.set(Duration::from_millis(200));
        assert_eq!(clock.now(), Duration::from_millis(500), "never backwards");
        clock.set(Duration::from_secs(2));
        assert_eq!(clock.now(), Duration::from_secs(2));
    }
}
