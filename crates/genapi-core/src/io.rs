//! Register access abstraction for GenApi transports.

use crate::GenApiError;

/// Register access abstraction backed by transports such as GVCP/GenCP.
pub trait RegisterIo {
    /// Read `len` bytes starting at `addr`.
    fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError>;
    /// Write `data` starting at `addr`.
    fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError>;
}

/// No-op register I/O that returns zeroed data for reads and ignores writes.
///
/// Useful for offline XML browsing (WASM, no camera) where register access
/// is not available. SwissKnife expressions depending only on XML-defined
/// constants will evaluate correctly; register-backed nodes will return zeros.
pub struct NullIo;

impl RegisterIo for NullIo {
    fn read(&self, _addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
        Ok(vec![0u8; len])
    }

    fn write(&self, _addr: u64, _data: &[u8]) -> Result<(), GenApiError> {
        Ok(())
    }
}
