//! GenICam camera service library — bridges viva-genicam to Zenoh for genicam-studio.
//!
//! The binary entrypoint lives in `main.rs`; this module re-exports the core
//! components so that integration tests can drive the service in-process.

pub mod acquisition;
pub mod config;
pub mod device;
pub mod nodes;
pub mod pixel_format;
pub mod status;
pub mod xml;
