//! Streaming statistics helpers.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const EWMA_ALPHA: f64 = 0.2;

/// Immutable view of streaming statistics suitable for UI overlays.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamStats {
    pub frames: u64,
    pub bytes: u64,
    pub drops: u64,
    pub resends: u64,
    pub last_frame_dt: Duration,
    pub avg_fps: f64,
    pub avg_mbps: f64,
    pub avg_latency_ms: Option<f64>,
    pub packets: u64,
    pub resend_ranges: u64,
    pub backpressure_drops: u64,
    pub late_frames: u64,
    pub pool_exhaustions: u64,
    pub elapsed: Duration,
    pub packets_per_second: f64,
}

impl Default for StreamStats {
    fn default() -> Self {
        StreamStats {
            frames: 0,
            bytes: 0,
            drops: 0,
            resends: 0,
            last_frame_dt: Duration::ZERO,
            avg_fps: 0.0,
            avg_mbps: 0.0,
            avg_latency_ms: None,
            packets: 0,
            resend_ranges: 0,
            backpressure_drops: 0,
            late_frames: 0,
            pool_exhaustions: 0,
            elapsed: Duration::ZERO,
            packets_per_second: 0.0,
        }
    }
}

#[derive(Debug)]
struct StatsState {
    frames: u64,
    bytes: u64,
    packets: u64,
    resends: u64,
    resend_ranges: u64,
    drops: u64,
    backpressure_drops: u64,
    late_frames: u64,
    pool_exhaustions: u64,
    last_frame_dt: Duration,
    avg_fps: f64,
    avg_mbps: f64,
    avg_latency_ms: Option<f64>,
    last_frame_instant: Option<Instant>,
    start: Instant,
}

impl StatsState {
    fn new() -> Self {
        Self {
            frames: 0,
            bytes: 0,
            packets: 0,
            resends: 0,
            resend_ranges: 0,
            drops: 0,
            backpressure_drops: 0,
            late_frames: 0,
            pool_exhaustions: 0,
            last_frame_dt: Duration::ZERO,
            avg_fps: 0.0,
            avg_mbps: 0.0,
            avg_latency_ms: None,
            last_frame_instant: None,
            start: Instant::now(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct StreamStatsAccumulator {
    inner: Arc<StatsInner>,
}

#[derive(Debug)]
struct StatsInner {
    state: Mutex<StatsState>,
}

impl StreamStatsAccumulator {
    /// Create a new statistics accumulator.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(StatsInner {
                state: Mutex::new(StatsState::new()),
            }),
        }
    }

    /// Record a received packet.
    pub fn record_packet(&self) {
        let mut state = self.inner.state.lock().expect("stats mutex poisoned");
        state.packets += 1;
    }

    /// Record a resend request.
    pub fn record_resend(&self) {
        let mut state = self.inner.state.lock().expect("stats mutex poisoned");
        state.resends += 1;
    }

    /// Record the number of packet ranges covered by a resend request.
    pub fn record_resend_ranges(&self, ranges: u64) {
        if ranges == 0 {
            return;
        }
        let mut state = self.inner.state.lock().expect("stats mutex poisoned");
        state.resend_ranges += ranges;
    }

    /// Record a dropped frame event.
    pub fn record_drop(&self) {
        let mut state = self.inner.state.lock().expect("stats mutex poisoned");
        state.drops += 1;
    }

    /// Record a drop caused by application backpressure.
    pub fn record_backpressure_drop(&self) {
        let mut state = self.inner.state.lock().expect("stats mutex poisoned");
        state.backpressure_drops += 1;
    }

    /// Record a frame that missed its presentation deadline.
    pub fn record_late_frame(&self) {
        let mut state = self.inner.state.lock().expect("stats mutex poisoned");
        state.late_frames += 1;
    }

    /// Record an exhausted frame buffer pool event.
    pub fn record_pool_exhaustion(&self) {
        let mut state = self.inner.state.lock().expect("stats mutex poisoned");
        state.pool_exhaustions += 1;
    }

    /// Update metrics for a fully received frame.
    pub fn record_frame(&self, bytes: usize, latency: Option<Duration>) {
        let now = Instant::now();
        let mut state = self.inner.state.lock().expect("stats mutex poisoned");
        state.frames += 1;
        state.bytes += bytes as u64;

        if let Some(prev) = state.last_frame_instant.replace(now) {
            let dt = now.saturating_duration_since(prev);
            if dt > Duration::ZERO {
                state.last_frame_dt = dt;
                let fps = 1.0 / dt.as_secs_f64();
                state.avg_fps = if state.avg_fps == 0.0 {
                    fps
                } else {
                    state.avg_fps + EWMA_ALPHA * (fps - state.avg_fps)
                };
                let mbps = (bytes as f64 * 8.0) / 1_000_000.0 / dt.as_secs_f64();
                state.avg_mbps = if state.avg_mbps == 0.0 {
                    mbps
                } else {
                    state.avg_mbps + EWMA_ALPHA * (mbps - state.avg_mbps)
                };
            }
        } else {
            state.last_frame_dt = Duration::ZERO;
        }

        if let Some(latency) = latency {
            let ms = latency.as_secs_f64() * 1_000.0;
            state.avg_latency_ms = Some(match state.avg_latency_ms {
                Some(prev) => prev + EWMA_ALPHA * (ms - prev),
                None => ms,
            });
        }
    }

    /// Produce a snapshot of the accumulated statistics.
    pub fn snapshot(&self) -> StreamStats {
        let state = self.inner.state.lock().expect("stats mutex poisoned");
        let elapsed = state.start.elapsed();
        let packets_per_second = if elapsed > Duration::ZERO {
            state.packets as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        };

        StreamStats {
            frames: state.frames,
            bytes: state.bytes,
            drops: state.drops + state.backpressure_drops,
            resends: state.resends,
            last_frame_dt: state.last_frame_dt,
            avg_fps: state.avg_fps,
            avg_mbps: state.avg_mbps,
            avg_latency_ms: state.avg_latency_ms,
            packets: state.packets,
            resend_ranges: state.resend_ranges,
            backpressure_drops: state.backpressure_drops,
            late_frames: state.late_frames,
            pool_exhaustions: state.pool_exhaustions,
            elapsed,
            packets_per_second,
        }
    }
}

impl Default for StreamStatsAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

/// Event channel statistics.
#[derive(Debug)]
pub struct EventStats {
    received: AtomicU64,
    malformed: AtomicU64,
    filtered: AtomicU64,
    start: Instant,
}

impl EventStats {
    /// Create a new accumulator for GVCP events.
    pub fn new() -> Self {
        Self {
            received: AtomicU64::new(0),
            malformed: AtomicU64::new(0),
            filtered: AtomicU64::new(0),
            start: Instant::now(),
        }
    }

    /// Record a successfully parsed event packet.
    pub fn record_event(&self) {
        self.received.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a dropped or malformed event packet.
    pub fn record_malformed(&self) {
        self.malformed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an event filtered out by the application.
    pub fn record_filtered(&self) {
        self.filtered.fetch_add(1, Ordering::Relaxed);
    }

    /// Snapshot the collected counters.
    pub fn snapshot(&self) -> EventSnapshot {
        EventSnapshot {
            received: self.received.load(Ordering::Relaxed),
            malformed: self.malformed.load(Ordering::Relaxed),
            filtered: self.filtered.load(Ordering::Relaxed),
            elapsed: self.start.elapsed().as_secs_f32(),
        }
    }
}

impl Default for EventStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Immutable view of event statistics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EventSnapshot {
    pub received: u64,
    pub malformed: u64,
    pub filtered: u64,
    pub elapsed: f32,
}

/// Action command dispatch statistics.
#[derive(Debug)]
pub struct ActionStats {
    sent: AtomicU64,
    acknowledgements: AtomicU64,
    failures: AtomicU64,
}

impl ActionStats {
    /// Create a new accumulator for action command metrics.
    pub fn new() -> Self {
        Self {
            sent: AtomicU64::new(0),
            acknowledgements: AtomicU64::new(0),
            failures: AtomicU64::new(0),
        }
    }

    /// Record a dispatched action.
    pub fn record_send(&self) {
        self.sent.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a received acknowledgement.
    pub fn record_ack(&self) {
        self.acknowledgements.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a failure while dispatching or waiting for acknowledgements.
    pub fn record_failure(&self) {
        self.failures.fetch_add(1, Ordering::Relaxed);
    }

    /// Snapshot the collected counters.
    pub fn snapshot(&self) -> ActionSnapshot {
        ActionSnapshot {
            sent: self.sent.load(Ordering::Relaxed),
            acknowledgements: self.acknowledgements.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed),
        }
    }
}

impl Default for ActionStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Immutable view of action statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionSnapshot {
    pub sent: u64,
    pub acknowledgements: u64,
    pub failures: u64,
}

/// Timestamp synchronisation statistics.
#[derive(Debug)]
pub struct TimeStats {
    samples: AtomicU64,
    latches: AtomicU64,
    resets: AtomicU64,
}

impl TimeStats {
    /// Create a new accumulator for timestamp operations.
    pub fn new() -> Self {
        Self {
            samples: AtomicU64::new(0),
            latches: AtomicU64::new(0),
            resets: AtomicU64::new(0),
        }
    }

    /// Record a calibration sample.
    pub fn record_sample(&self) {
        self.samples.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a timestamp latch request.
    pub fn record_latch(&self) {
        self.latches.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a timestamp reset operation.
    pub fn record_reset(&self) {
        self.resets.fetch_add(1, Ordering::Relaxed);
    }

    /// Snapshot the current counters.
    pub fn snapshot(&self) -> TimeSnapshot {
        TimeSnapshot {
            samples: self.samples.load(Ordering::Relaxed),
            latches: self.latches.load(Ordering::Relaxed),
            resets: self.resets.load(Ordering::Relaxed),
        }
    }
}

impl Default for TimeStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Immutable view of timestamp statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSnapshot {
    pub samples: u64,
    pub latches: u64,
    pub resets: u64,
}
