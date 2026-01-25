//! Device timestamp helpers and host mapping utilities.
//!
//! The [`TimeSync`] struct maintains a sliding window of timestamp measurements
//! and computes a linear model mapping device ticks to host time. It supports:
//! - Configurable window capacity
//! - Optional outlier trimming for robustness
//! - Both immediate (auto-fit on update) and deferred (manual fit) modes

use std::cmp::Ordering;
use std::collections::VecDeque;
use std::convert::TryInto;
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use thiserror::Error;
use tracing::trace;

use crate::gvcp::GigeError;

/// Address of the SFNC `TimestampControl` register.
pub const REG_TIMESTAMP_CONTROL: u64 = 0x0900_0100;
/// Address of the SFNC `TimestampValue` register (64-bit).
pub const REG_TIMESTAMP_VALUE: u64 = 0x0900_0104;
/// Address of the SFNC `TimestampTickFrequency` register (64-bit).
pub const REG_TIMESTAMP_TICK_FREQUENCY: u64 = 0x0900_010C;
/// Bit flag to latch the timestamp counter.
pub const TIMESTAMP_LATCH_BIT: u32 = 0x0000_0002;
/// Bit flag to reset the timestamp counter.
pub const TIMESTAMP_RESET_BIT: u32 = 0x0000_0001;
/// Default maximum number of samples kept for linear regression.
pub const DEFAULT_TIME_WINDOW: usize = 32;

/// Errors encountered while interacting with timestamp control registers.
#[derive(Debug, Error)]
pub enum TimeError {
    #[error("control: {0}")]
    Control(#[from] GigeError),
    #[error("protocol: {0}")]
    Protocol(&'static str),
}

/// Minimal interface required to read/write timestamp registers.
#[async_trait]
pub trait ControlChannel: Send + Sync {
    async fn read_register(&self, addr: u64, len: usize) -> Result<Vec<u8>, TimeError>;
    async fn write_register(&self, addr: u64, data: &[u8]) -> Result<(), TimeError>;
}

fn write_u32_be(value: u32) -> [u8; 4] {
    value.to_be_bytes()
}

fn parse_u64_be(data: &[u8]) -> Result<u64, TimeError> {
    if data.len() != 8 {
        return Err(TimeError::Protocol("unexpected register size"));
    }
    Ok(u64::from_be_bytes(
        data.try_into().expect("slice length checked"),
    ))
}

/// Issue a timestamp reset using the SFNC control register.
pub async fn timestamp_reset<C: ControlChannel>(ctrl: &C) -> Result<(), TimeError> {
    trace!("triggering timestamp reset");
    ctrl.write_register(REG_TIMESTAMP_CONTROL, &write_u32_be(TIMESTAMP_RESET_BIT))
        .await
}

/// Latch the current timestamp counter to make it readable without jitter.
pub async fn timestamp_latch<C: ControlChannel>(ctrl: &C) -> Result<(), TimeError> {
    trace!("triggering timestamp latch");
    ctrl.write_register(REG_TIMESTAMP_CONTROL, &write_u32_be(TIMESTAMP_LATCH_BIT))
        .await
}

/// Read the current 64-bit timestamp value from the device.
pub async fn read_timestamp_value<C: ControlChannel>(ctrl: &C) -> Result<u64, TimeError> {
    let bytes = ctrl.read_register(REG_TIMESTAMP_VALUE, 8).await?;
    parse_u64_be(&bytes)
}

/// Read the device tick frequency.
pub async fn read_tick_frequency<C: ControlChannel>(ctrl: &C) -> Result<u64, TimeError> {
    let bytes = ctrl.read_register(REG_TIMESTAMP_TICK_FREQUENCY, 8).await?;
    parse_u64_be(&bytes)
}

/// Maintain a linear mapping between device ticks and host time.
///
/// This struct collects timestamp measurement pairs and uses linear regression
/// to compute a mapping from device ticks to host time. It supports:
/// - Configurable window size (number of samples to retain)
/// - Optional outlier trimming for robustness against jitter
/// - Both auto-fit (recompute on every update) and manual fit modes
#[derive(Debug, Clone)]
pub struct TimeSync {
    /// Linear fit slope (seconds per tick).
    a: f64,
    /// Linear fit intercept (seconds).
    b: f64,
    /// Sample window storing device ticks and host instants.
    window: VecDeque<(u64, Instant)>,
    /// Maximum number of samples retained in the window.
    cap: usize,
    /// Host instant corresponding to the first recorded sample.
    origin_instant: Option<Instant>,
    /// Host system time captured alongside the origin instant.
    origin_system: Option<SystemTime>,
    /// Optional device tick frequency when reported by the camera.
    freq_hz: Option<f64>,
    /// Whether to automatically recompute fit on every update.
    auto_fit: bool,
    /// Whether to trim outliers when fitting (10% from each end when n≥10).
    trim_outliers: bool,
}

impl TimeSync {
    /// Create an empty synchroniser with default capacity and auto-fit enabled.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_TIME_WINDOW)
    }

    /// Create a synchroniser with custom capacity.
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            a: 0.0,
            b: 0.0,
            window: VecDeque::with_capacity(cap),
            cap,
            origin_instant: None,
            origin_system: None,
            freq_hz: None,
            auto_fit: true,
            trim_outliers: false,
        }
    }

    /// Enable or disable automatic refitting on every update.
    ///
    /// When disabled, you must call [`fit`](Self::fit) manually to update coefficients.
    pub fn set_auto_fit(&mut self, enabled: bool) -> &mut Self {
        self.auto_fit = enabled;
        self
    }

    /// Enable or disable outlier trimming during fit.
    ///
    /// When enabled, the 10% most extreme residuals are excluded from
    /// the regression when at least 10 samples are available.
    pub fn set_trim_outliers(&mut self, enabled: bool) -> &mut Self {
        self.trim_outliers = enabled;
        self
    }

    /// Return the current slope and intercept of the time mapping.
    pub fn coefficients(&self) -> (f64, f64) {
        (self.a, self.b)
    }

    /// Number of samples retained in the sliding window.
    pub fn len(&self) -> usize {
        self.window.len()
    }

    /// Check if samples window is empty.
    pub fn is_empty(&self) -> bool {
        self.window.is_empty()
    }

    /// Iterator over the samples contained in the sliding window.
    pub fn samples(&self) -> impl Iterator<Item = (u64, Instant)> + '_ {
        self.window.iter().copied()
    }

    /// Maximum number of samples stored in the sliding window.
    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// Access the origin instant if at least one sample has been recorded.
    pub fn origin_instant(&self) -> Option<Instant> {
        self.origin_instant
    }

    /// Access the origin system time if available.
    pub fn origin_system(&self) -> Option<SystemTime> {
        self.origin_system
    }

    /// Return the first and last sample retained in the window.
    pub fn sample_bounds(&self) -> Option<((u64, Instant), (u64, Instant))> {
        let first = *self.window.front()?;
        let last = *self.window.back()?;
        Some((first, last))
    }

    /// Retrieve the reported device tick frequency.
    pub fn freq_hz(&self) -> Option<f64> {
        self.freq_hz
    }

    /// Set the device tick frequency.
    pub fn set_freq_hz(&mut self, freq: f64) {
        self.freq_hz = Some(freq);
    }

    /// Add a new measurement pair to the regression window.
    ///
    /// If auto-fit is enabled (default), the linear model is recomputed immediately.
    pub fn update(&mut self, dev_ts: u64, host_instant: Instant) {
        if self.origin_instant.is_none() {
            self.origin_instant = Some(host_instant);
            self.origin_system = Some(SystemTime::now());
        }
        if self.window.len() == self.cap {
            self.window.pop_front();
        }
        self.window.push_back((dev_ts, host_instant));
        if self.auto_fit {
            self.recompute();
        }
    }

    /// Fit the linear model, optionally updating the frequency.
    ///
    /// Returns the updated `(slope, intercept)` coefficients when enough samples
    /// are available, or `None` if fewer than 2 samples exist.
    pub fn fit(&mut self, freq_hz: Option<f64>) -> Option<(f64, f64)> {
        if let Some(freq) = freq_hz {
            self.freq_hz = Some(freq);
        }
        self.recompute();
        if self.window.len() >= 2 {
            Some((self.a, self.b))
        } else {
            None
        }
    }

    fn recompute(&mut self) {
        if self.window.len() < 2 {
            return;
        }
        let origin = match self.origin_instant {
            Some(o) => o,
            None => return,
        };
        let base_tick = match self.window.front() {
            Some((t, _)) => *t as f64,
            None => return,
        };

        let samples: Vec<(f64, f64)> = self
            .window
            .iter()
            .map(|(ticks, host)| {
                let x = (*ticks as f64) - base_tick;
                let y = host.duration_since(origin).as_secs_f64();
                (x, y)
            })
            .collect();

        let (mut slope, mut intercept_rel) = match compute_fit(&samples) {
            Some((s, i)) => (s, i),
            None => return,
        };

        // Apply outlier trimming if enabled and we have enough samples
        if self.trim_outliers && samples.len() >= 10 {
            let mut residuals: Vec<(usize, f64)> = samples
                .iter()
                .enumerate()
                .map(|(idx, (x, y))| {
                    let predicted = slope * *x + intercept_rel;
                    (idx, y - predicted)
                })
                .collect();
            residuals.sort_by(|a, b| match a.1.partial_cmp(&b.1) {
                Some(order) => order,
                None => Ordering::Equal,
            });
            let trim = ((residuals.len() as f64) * 0.1).floor() as usize;
            if trim > 0 && residuals.len() > trim * 2 {
                let trimmed_samples: Vec<(f64, f64)> = residuals[trim..residuals.len() - trim]
                    .iter()
                    .map(|(idx, _)| samples[*idx])
                    .collect();
                if let Some((s, i)) = compute_fit(&trimmed_samples) {
                    slope = s;
                    intercept_rel = i;
                }
            }
        }

        let intercept = intercept_rel - slope * base_tick;
        self.a = slope;
        self.b = intercept;

        trace!(
            slope = self.a,
            intercept = self.b,
            samples = self.window.len(),
            "recomputed time mapping"
        );
    }

    /// Convert a device timestamp into a host `SystemTime`.
    pub fn to_host_time(&self, dev_ts: u64) -> SystemTime {
        let origin = match self.origin_system {
            Some(o) => o,
            None => return SystemTime::now(),
        };
        let seconds = self.a * dev_ts as f64 + self.b;
        if seconds.is_finite() && seconds >= 0.0 {
            match Duration::try_from_secs_f64(seconds) {
                Ok(duration) => origin + duration,
                Err(_) => origin,
            }
        } else {
            origin
        }
    }
}

impl Default for TimeSync {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute linear regression slope and intercept from samples.
fn compute_fit(samples: &[(f64, f64)]) -> Option<(f64, f64)> {
    if samples.len() < 2 {
        return None;
    }
    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    for (x, y) in samples {
        sum_x += x;
        sum_y += y;
    }
    let n = samples.len() as f64;
    let mean_x = sum_x / n;
    let mean_y = sum_y / n;
    let mut denom = 0.0;
    let mut numer = 0.0;
    for (x, y) in samples {
        let dx = x - mean_x;
        let dy = y - mean_y;
        denom += dx * dx;
        numer += dx * dy;
    }
    if denom.abs() < f64::EPSILON {
        return None;
    }
    let slope = numer / denom;
    let intercept = mean_y - slope * mean_x;
    Some((slope, intercept))
}

#[async_trait]
impl ControlChannel for tokio::sync::Mutex<crate::gvcp::GigeDevice> {
    async fn read_register(&self, addr: u64, len: usize) -> Result<Vec<u8>, TimeError> {
        let mut guard = self.lock().await;
        guard.read_mem(addr, len).await.map_err(TimeError::from)
    }

    async fn write_register(&self, addr: u64, data: &[u8]) -> Result<(), TimeError> {
        let mut guard = self.lock().await;
        guard.write_mem(addr, data).await.map_err(TimeError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression_tracks_linear_relation() {
        let mut sync = TimeSync::new();
        let start = Instant::now();
        for i in 0..16u64 {
            let dev = i * 1000;
            let host = start + Duration::from_millis(i * 16);
            sync.update(dev, host);
        }
        let mapped = sync.to_host_time(64_000);
        let origin = sync.origin_system().unwrap();
        let mapped_secs = mapped.duration_since(origin).unwrap().as_secs_f64();
        let expected_secs = Duration::from_millis(1024).as_secs_f64();
        assert!((mapped_secs - expected_secs).abs() < 0.1);
    }

    #[test]
    fn with_capacity_and_manual_fit() {
        let mut sync = TimeSync::with_capacity(8);
        sync.set_auto_fit(false);
        let start = Instant::now();
        for i in 0..8u64 {
            let dev = i * 1000;
            let host = start + Duration::from_millis(i * 10);
            sync.update(dev, host);
        }
        // Coefficients should still be default since auto_fit is off
        let (a, _) = sync.coefficients();
        assert_eq!(a, 0.0);

        // Now fit manually
        let result = sync.fit(Some(100_000.0));
        assert!(result.is_some());
        let (a, _) = sync.coefficients();
        assert!(a > 0.0);
        assert_eq!(sync.freq_hz(), Some(100_000.0));
    }

    #[test]
    fn outlier_trimming() {
        let mut sync = TimeSync::with_capacity(32);
        sync.set_trim_outliers(true).set_auto_fit(false);
        let start = Instant::now();
        // Add samples with one outlier
        for i in 0..20u64 {
            let dev = i * 1000;
            let jitter = if i == 10 { 50 } else { 0 }; // outlier at i=10
            let host = start + Duration::from_millis(i * 10 + jitter);
            sync.update(dev, host);
        }
        sync.fit(None);
        // With trimming, the fit should be more accurate
        assert_eq!(sync.len(), 20);
    }
}
