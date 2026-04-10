//! USB transfer abstraction for testability.
//!
//! [`UsbTransfer`] decouples all U3V protocol logic from the `rusb` crate,
//! allowing the control channel, streaming, and bootstrap parsing to be
//! tested with [`MockUsbTransfer`] — no USB hardware required.

use std::time::Duration;

use crate::U3vError;

/// Abstraction over a claimed USB device for bulk endpoint I/O.
///
/// Implementations must be safe to share across threads. The control and
/// stream channels use different endpoints, so a single `UsbTransfer` can
/// serve both concurrently (with internal synchronization if needed).
pub trait UsbTransfer: Send + Sync {
    /// Write `data` to the bulk OUT `endpoint`. Returns bytes written.
    fn bulk_write(&self, endpoint: u8, data: &[u8], timeout: Duration) -> Result<usize, U3vError>;

    /// Read up to `buf.len()` bytes from the bulk IN `endpoint`.
    /// Returns the number of bytes actually read.
    fn bulk_read(&self, endpoint: u8, buf: &mut [u8], timeout: Duration)
        -> Result<usize, U3vError>;
}

// ---------------------------------------------------------------------------
// rusb-backed implementation (behind `usb` feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "usb")]
pub use self::rusb_impl::RusbTransfer;

#[cfg(feature = "usb")]
mod rusb_impl {
    use super::*;
    use std::sync::Arc;

    /// [`UsbTransfer`] backed by a real `rusb::DeviceHandle`.
    pub struct RusbTransfer {
        handle: Arc<rusb::DeviceHandle<rusb::Context>>,
    }

    impl RusbTransfer {
        /// Wrap an already-opened and claimed device handle.
        pub fn new(handle: Arc<rusb::DeviceHandle<rusb::Context>>) -> Self {
            Self { handle }
        }
    }

    impl UsbTransfer for RusbTransfer {
        fn bulk_write(
            &self,
            endpoint: u8,
            data: &[u8],
            timeout: Duration,
        ) -> Result<usize, U3vError> {
            self.handle
                .write_bulk(endpoint, data, timeout)
                .map_err(|e| U3vError::Usb(e.to_string()))
        }

        fn bulk_read(
            &self,
            endpoint: u8,
            buf: &mut [u8],
            timeout: Duration,
        ) -> Result<usize, U3vError> {
            self.handle
                .read_bulk(endpoint, buf, timeout)
                .map_err(|e| U3vError::Usb(e.to_string()))
        }
    }
}

// ---------------------------------------------------------------------------
// Mock implementation for testing
// ---------------------------------------------------------------------------

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};

/// In-memory mock of [`UsbTransfer`] for unit tests.
///
/// Pre-load expected read responses per endpoint, then execute protocol
/// logic. After the test, inspect captured writes to verify correctness.
///
/// # Thread safety
///
/// Uses interior mutability via `RefCell` — intended for single-threaded
/// test contexts only. Wrap in a `Mutex` if concurrent access is needed.
pub struct MockUsbTransfer {
    /// Queued read responses per endpoint address.
    reads: RefCell<HashMap<u8, VecDeque<Vec<u8>>>>,
    /// Captured write payloads per endpoint address.
    writes: RefCell<HashMap<u8, Vec<Vec<u8>>>>,
}

// SAFETY: MockUsbTransfer is only used in single-threaded test contexts.
// The UsbTransfer trait requires Send + Sync, so we provide the bounds.
unsafe impl Send for MockUsbTransfer {}
unsafe impl Sync for MockUsbTransfer {}

impl MockUsbTransfer {
    /// Create an empty mock with no pre-loaded responses.
    pub fn new() -> Self {
        Self {
            reads: RefCell::new(HashMap::new()),
            writes: RefCell::new(HashMap::new()),
        }
    }

    /// Enqueue a response that will be returned by the next `bulk_read`
    /// on the given `endpoint`.
    pub fn enqueue_read(&self, endpoint: u8, data: Vec<u8>) {
        self.reads
            .borrow_mut()
            .entry(endpoint)
            .or_default()
            .push_back(data);
    }

    /// Return all captured write payloads for the given `endpoint`.
    pub fn take_writes(&self, endpoint: u8) -> Vec<Vec<u8>> {
        self.writes
            .borrow_mut()
            .remove(&endpoint)
            .unwrap_or_default()
    }
}

impl Default for MockUsbTransfer {
    fn default() -> Self {
        Self::new()
    }
}

impl UsbTransfer for MockUsbTransfer {
    fn bulk_write(&self, endpoint: u8, data: &[u8], _timeout: Duration) -> Result<usize, U3vError> {
        let len = data.len();
        self.writes
            .borrow_mut()
            .entry(endpoint)
            .or_default()
            .push(data.to_vec());
        Ok(len)
    }

    fn bulk_read(
        &self,
        endpoint: u8,
        buf: &mut [u8],
        _timeout: Duration,
    ) -> Result<usize, U3vError> {
        let mut reads = self.reads.borrow_mut();
        let queue = reads.get_mut(&endpoint).ok_or_else(|| {
            U3vError::Protocol(format!("no queued read for endpoint {endpoint:#04x}"))
        })?;
        let data = queue.pop_front().ok_or_else(|| {
            U3vError::Protocol(format!("read queue exhausted for endpoint {endpoint:#04x}"))
        })?;
        let n = data.len().min(buf.len());
        buf[..n].copy_from_slice(&data[..n]);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_write_then_read() {
        let mock = MockUsbTransfer::new();
        let ep_out = 0x01;
        let ep_in = 0x81;

        // Write some data
        let written = mock
            .bulk_write(ep_out, &[1, 2, 3], Duration::from_millis(100))
            .unwrap();
        assert_eq!(written, 3);

        // Enqueue a read response and read it back
        mock.enqueue_read(ep_in, vec![4, 5, 6, 7]);
        let mut buf = [0u8; 8];
        let n = mock
            .bulk_read(ep_in, &mut buf, Duration::from_millis(100))
            .unwrap();
        assert_eq!(n, 4);
        assert_eq!(&buf[..4], &[4, 5, 6, 7]);

        // Verify captured writes
        let writes = mock.take_writes(ep_out);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0], &[1, 2, 3]);
    }

    #[test]
    fn mock_read_exhausted_returns_error() {
        let mock = MockUsbTransfer::new();
        let mut buf = [0u8; 4];
        let err = mock
            .bulk_read(0x81, &mut buf, Duration::from_millis(100))
            .unwrap_err();
        assert!(matches!(err, U3vError::Protocol(_)));
    }
}
