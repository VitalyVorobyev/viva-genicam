//! Counters for one GVSP stream (SR-07), written by its receive engine.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const EWMA_ALPHA: f64 = 0.2;

/// Snapshot of one GVSP stream's counters (backlog SR-07).
///
/// **Frames.** Every block the camera sends is counted exactly once, when its
/// fate is decided, so that
///
/// ```text
/// blocks sent = frames_complete + frames_incomplete + frames_missed
///             + frames_backpressure + frames_unsupported
/// ```
///
/// once nothing is in flight and the delivery queue is drained. A frame
/// waiting in the queue is in none of them yet.
///
/// **Packets.** `packets_received` counts every datagram read from the socket;
/// the other packet counters say what became of the ones that were not used.
///
/// Resend counters arrive with packet resend (milestone S); until something
/// can increment them they are not here.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct StreamStats {
    /// Frames handed to the application with every packet present.
    pub frames_complete: u64,
    /// Frames closed without that: packets missing, no trailer, or an error
    /// status from the device. Counted whether or not the stream delivers
    /// incomplete frames.
    pub frames_incomplete: u64,
    /// Blocks never seen at all: gaps in the block-id sequence.
    pub frames_missed: u64,
    /// Frames assembled and then discarded because the application did not
    /// take them before the delivery queue filled.
    pub frames_backpressure: u64,
    /// Blocks whose payload type the engine does not assemble.
    pub frames_unsupported: u64,
    /// Datagrams read from the socket.
    pub packets_received: u64,
    /// Packets received more than once.
    pub packets_duplicate: u64,
    /// Packets never received, of frames closed incomplete. A lower bound when
    /// a trailer is missing, since the trailer is what says how many there were.
    pub packets_missing: u64,
    /// Datagrams that are not GVSP.
    pub packets_ignored_malformed: u64,
    /// Packets of a format or payload type the engine does not assemble.
    pub packets_ignored_unsupported: u64,
    /// Data packets beyond the end of the frame buffer.
    pub packets_ignored_out_of_range: u64,
    /// Packets for a block already closed or older than the ones in flight.
    pub packets_ignored_late: u64,
    /// Leader or data packets whose status reports an error.
    pub packets_ignored_error_status: u64,
    /// Data packets, other than a block's last, whose length is not the
    /// stride the packet size implies.
    pub packets_size_mismatch: u64,
    /// Payload bytes of complete frames handed to the application.
    pub bytes: u64,
    /// Time between the last two complete frames.
    pub last_frame_dt: Duration,
    /// Smoothed rate of complete frames.
    pub avg_fps: f64,
    /// Smoothed payload throughput of complete frames, in megabits a second.
    pub avg_mbps: f64,
    /// Smoothed delay from the device timestamp to delivery, when the stream
    /// can map device time to host time.
    pub avg_latency_ms: Option<f64>,
    /// Time since the stream started.
    pub elapsed: Duration,
    /// Average datagram rate since the stream started.
    pub packets_per_second: f64,
}

impl StreamStats {
    /// Frames the application did not receive complete: incomplete, missed
    /// and dropped to backpressure.
    pub fn frames_lost(&self) -> u64 {
        self.frames_incomplete + self.frames_missed + self.frames_backpressure
    }

    /// Packets received and not used, for any reason.
    pub fn packets_ignored(&self) -> u64 {
        self.packets_ignored_malformed
            + self.packets_ignored_unsupported
            + self.packets_ignored_out_of_range
            + self.packets_ignored_late
            + self.packets_ignored_error_status
    }
}

/// Why a received packet was not used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum IgnoredPacket {
    /// Not a GVSP packet.
    Malformed,
    /// A packet format or payload type the engine does not assemble.
    Unsupported,
    /// A data packet beyond the end of the frame buffer.
    OutOfRange,
    /// For a block already closed, or older than the blocks in flight.
    Late,
    /// A leader or data packet whose status reports an error.
    ErrorStatus,
}

#[derive(Debug)]
struct Rates {
    last_frame_dt: Duration,
    avg_fps: f64,
    avg_mbps: f64,
    avg_latency_ms: Option<f64>,
    last_frame_instant: Option<Instant>,
}

#[derive(Debug)]
struct StatsInner {
    start: Instant,
    frames_complete: AtomicU64,
    frames_incomplete: AtomicU64,
    frames_missed: AtomicU64,
    frames_backpressure: AtomicU64,
    frames_unsupported: AtomicU64,
    packets_received: AtomicU64,
    packets_duplicate: AtomicU64,
    packets_missing: AtomicU64,
    ignored_malformed: AtomicU64,
    ignored_unsupported: AtomicU64,
    ignored_out_of_range: AtomicU64,
    ignored_late: AtomicU64,
    ignored_error_status: AtomicU64,
    packets_size_mismatch: AtomicU64,
    bytes: AtomicU64,
    rates: Mutex<Rates>,
}

/// Shared, cloneable handle to one stream's counters.
///
/// The receive engine writes them, packet by packet, from its own thread; any
/// clone can [`snapshot`](Self::snapshot) them, from any thread.
#[derive(Debug, Clone)]
pub struct StreamStatsAccumulator {
    inner: Arc<StatsInner>,
}

fn add(counter: &AtomicU64, n: u64) {
    counter.fetch_add(n, Ordering::Relaxed);
}

impl StreamStatsAccumulator {
    /// Create a new statistics accumulator.
    pub(crate) fn new() -> Self {
        let zero = || AtomicU64::new(0);
        Self {
            inner: Arc::new(StatsInner {
                start: Instant::now(),
                frames_complete: zero(),
                frames_incomplete: zero(),
                frames_missed: zero(),
                frames_backpressure: zero(),
                frames_unsupported: zero(),
                packets_received: zero(),
                packets_duplicate: zero(),
                packets_missing: zero(),
                ignored_malformed: zero(),
                ignored_unsupported: zero(),
                ignored_out_of_range: zero(),
                ignored_late: zero(),
                ignored_error_status: zero(),
                packets_size_mismatch: zero(),
                bytes: zero(),
                rates: Mutex::new(Rates {
                    last_frame_dt: Duration::ZERO,
                    avg_fps: 0.0,
                    avg_mbps: 0.0,
                    avg_latency_ms: None,
                    last_frame_instant: None,
                }),
            }),
        }
    }

    /// A datagram was read from the socket.
    pub(crate) fn record_packet_received(&self) {
        add(&self.inner.packets_received, 1);
    }

    /// A packet arrived that had already been received.
    pub(crate) fn record_duplicate_packet(&self) {
        add(&self.inner.packets_duplicate, 1);
    }

    /// A packet was received and not used.
    pub(crate) fn record_ignored_packet(&self, reason: IgnoredPacket) {
        let counter = match reason {
            IgnoredPacket::Malformed => &self.inner.ignored_malformed,
            IgnoredPacket::Unsupported => &self.inner.ignored_unsupported,
            IgnoredPacket::OutOfRange => &self.inner.ignored_out_of_range,
            IgnoredPacket::Late => &self.inner.ignored_late,
            IgnoredPacket::ErrorStatus => &self.inner.ignored_error_status,
        };
        add(counter, 1);
    }

    /// A data packet other than a block's last did not fill the stride.
    pub(crate) fn record_size_mismatch(&self) {
        add(&self.inner.packets_size_mismatch, 1);
    }

    /// A frame closed incomplete without `count` of its packets.
    pub(crate) fn record_missing_packets(&self, count: u64) {
        add(&self.inner.packets_missing, count);
    }

    /// A complete frame of `bytes` was handed to the application.
    pub(crate) fn record_frame(&self, bytes: usize, latency: Option<Duration>) {
        add(&self.inner.frames_complete, 1);
        add(&self.inner.bytes, bytes as u64);
        let now = Instant::now();
        let mut rates = self.inner.rates.lock().expect("stats mutex poisoned");
        if let Some(prev) = rates.last_frame_instant.replace(now) {
            let dt = now.saturating_duration_since(prev);
            if dt > Duration::ZERO {
                rates.last_frame_dt = dt;
                let fps = 1.0 / dt.as_secs_f64();
                rates.avg_fps = if rates.avg_fps == 0.0 {
                    fps
                } else {
                    rates.avg_fps + EWMA_ALPHA * (fps - rates.avg_fps)
                };
                let mbps = (bytes as f64 * 8.0) / 1_000_000.0 / dt.as_secs_f64();
                rates.avg_mbps = if rates.avg_mbps == 0.0 {
                    mbps
                } else {
                    rates.avg_mbps + EWMA_ALPHA * (mbps - rates.avg_mbps)
                };
            }
        } else {
            rates.last_frame_dt = Duration::ZERO;
        }
        if let Some(latency) = latency {
            let ms = latency.as_secs_f64() * 1_000.0;
            rates.avg_latency_ms = Some(match rates.avg_latency_ms {
                Some(prev) => prev + EWMA_ALPHA * (ms - prev),
                None => ms,
            });
        }
    }

    /// A frame closed without every packet, or with an error status.
    pub(crate) fn record_incomplete_frame(&self) {
        add(&self.inner.frames_incomplete, 1);
    }

    /// `count` block ids were skipped.
    pub(crate) fn record_missed_frames(&self, count: u64) {
        add(&self.inner.frames_missed, count);
    }

    /// An assembled frame was discarded because the delivery queue was full.
    pub(crate) fn record_backpressure_drop(&self) {
        add(&self.inner.frames_backpressure, 1);
    }

    /// A block arrived whose payload type the engine does not assemble.
    pub(crate) fn record_unsupported_block(&self) {
        add(&self.inner.frames_unsupported, 1);
    }

    /// Produce a snapshot of the accumulated statistics.
    pub fn snapshot(&self) -> StreamStats {
        let get = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        let inner = &*self.inner;
        let elapsed = inner.start.elapsed();
        let packets_received = get(&inner.packets_received);
        let packets_per_second = if elapsed > Duration::ZERO {
            packets_received as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        };
        let rates = inner.rates.lock().expect("stats mutex poisoned");
        StreamStats {
            frames_complete: get(&inner.frames_complete),
            frames_incomplete: get(&inner.frames_incomplete),
            frames_missed: get(&inner.frames_missed),
            frames_backpressure: get(&inner.frames_backpressure),
            frames_unsupported: get(&inner.frames_unsupported),
            packets_received,
            packets_duplicate: get(&inner.packets_duplicate),
            packets_missing: get(&inner.packets_missing),
            packets_ignored_malformed: get(&inner.ignored_malformed),
            packets_ignored_unsupported: get(&inner.ignored_unsupported),
            packets_ignored_out_of_range: get(&inner.ignored_out_of_range),
            packets_ignored_late: get(&inner.ignored_late),
            packets_ignored_error_status: get(&inner.ignored_error_status),
            packets_size_mismatch: get(&inner.packets_size_mismatch),
            bytes: get(&inner.bytes),
            last_frame_dt: rates.last_frame_dt,
            avg_fps: rates.avg_fps,
            avg_mbps: rates.avg_mbps,
            avg_latency_ms: rates.avg_latency_ms,
            elapsed,
            packets_per_second,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_are_independent_and_sum_as_documented() {
        let stats = StreamStatsAccumulator::new();
        stats.record_frame(100, None);
        stats.record_frame(100, None);
        stats.record_incomplete_frame();
        stats.record_missed_frames(3);
        stats.record_backpressure_drop();
        stats.record_ignored_packet(IgnoredPacket::Late);
        stats.record_ignored_packet(IgnoredPacket::Malformed);
        stats.record_duplicate_packet();
        let snap = stats.clone().snapshot();
        assert_eq!(snap.frames_complete, 2);
        assert_eq!(snap.bytes, 200);
        assert_eq!(snap.frames_lost(), 1 + 3 + 1);
        assert_eq!(snap.packets_ignored(), 2);
        assert_eq!(snap.packets_duplicate, 1);
        assert_eq!(snap.packets_ignored_late, 1);
    }
}
