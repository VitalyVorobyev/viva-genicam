//! Re-exports the canonical TimeSync implementation from tl-gige.
//!
//! The [`TimeSync`] struct maintains a sliding window of timestamp measurements
//! and computes a linear model mapping device ticks to host time. See the
//! `viva_gige::time` module for full documentation.

pub use viva_gige::time::{DEFAULT_TIME_WINDOW, TimeSync};
