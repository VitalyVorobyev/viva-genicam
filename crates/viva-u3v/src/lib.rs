//! USB3 Vision transport layer.
//!
//! Provides device discovery, GenCP-over-USB control, bootstrap register
//! parsing, and bulk-endpoint streaming for USB3 Vision cameras.
//!
//! The `usb` feature flag enables actual USB hardware access via `rusb`.
//! Without it, only protocol types, encoding, and mock-based testing compile.

pub mod bootstrap;
pub mod control;
pub mod descriptor;
pub mod device;
pub mod discovery;
pub mod stream;
pub mod usb;

use viva_gencp::StatusCode;

/// Errors produced by the USB3 Vision transport layer.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum U3vError {
    /// Low-level USB I/O error.
    #[error("usb: {0}")]
    Usb(String),

    /// Wire-format or protocol violation.
    #[error("protocol: {0}")]
    Protocol(String),

    /// A bulk transfer or control transaction timed out.
    #[error("timeout")]
    Timeout,

    /// GenCP-level decode error.
    #[error("gencp: {0}")]
    GenCp(#[from] viva_gencp::GenCpError),

    /// Device returned a non-success status code.
    #[error("device status: {status:?}")]
    Status { status: StatusCode },

    /// USB descriptor parsing failed.
    #[error("descriptor: {0}")]
    Descriptor(String),
}
