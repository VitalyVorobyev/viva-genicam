//! Register access abstraction for GenApi transports.

use crate::GenApiError;

/// Register access abstraction backed by transports such as GVCP/GenCP.
pub trait RegisterIo {
    /// Read `len` bytes starting at `addr`.
    fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError>;
    /// Write `data` starting at `addr`.
    fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError>;
}
