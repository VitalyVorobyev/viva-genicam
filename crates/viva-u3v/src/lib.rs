//! USB3 Vision transport layer (placeholder).

#![allow(dead_code)]

/// Placeholder error type until tl-u3v is implemented.
#[derive(Debug, thiserror::Error)]
pub enum U3vError {
    #[error("unimplemented")]
    Unimplemented,
}
