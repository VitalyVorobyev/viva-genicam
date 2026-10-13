//! USB transfer abstraction for testability.
//!
//! [`UsbTransfer`] decouples all U3V protocol logic from the USB library,
//! allowing the control channel, streaming, and bootstrap parsing to be
//! tested with [`MockUsbTransfer`] and `viva-fake-u3v` — no USB hardware
//! required. [`BulkIn`] is how a stream keeps several transfers in flight
//! (backlog UV-01): a transport that can queue them, as `nusb` does,
//! implements it; any other gets one read at a time through
//! [`UsbTransfer::bulk_read`].

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::U3vError;
use crate::bootstrap::UsbSpeed;

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

    /// Reads on the bulk IN `endpoint` that can be queued ahead of the
    /// device, as a stream needs them.
    ///
    /// The default reads one transfer at a time through
    /// [`bulk_read`](Self::bulk_read), when the next completion is asked for:
    /// correct, but with nothing queued while the host is busy elsewhere. A
    /// transport that can keep transfers in flight overrides it.
    fn bulk_in(self: &Arc<Self>, endpoint: u8) -> Result<Box<dyn BulkIn>, U3vError>
    where
        Self: Sized + 'static,
    {
        Ok(Box::new(Unqueued {
            transport: Arc::clone(self),
            endpoint,
            requested: VecDeque::new(),
            cancelled: 0,
        }))
    }

    /// The largest packet the endpoint carries, from its descriptor; `None`
    /// when unknown. Every IN transfer a stream asks for is a multiple of
    /// it, so that a packet never straddles two transfers.
    fn max_packet_size(&self, _endpoint: u8) -> Option<usize> {
        None
    }

    /// The speed the host's USB stack reports the device runs at; `None`
    /// when this transport cannot tell.
    fn link_speed(&self) -> Option<UsbSpeed> {
        None
    }

    /// Whether the operating system still lists the device, asked after a
    /// transfer failed in a way that does not say whether it is gone;
    /// `None` when this transport cannot tell. One look, no waiting.
    fn is_present(&self) -> Option<bool> {
        None
    }
}

/// How long a device whose transfer failed gets to leave the operating
/// system's list before it counts as still attached. On macOS the transfer
/// in flight fails before the unplugged device leaves the registry; an
/// mvBlueFOX3-2004G's next control transfer failed 3 ms later.
pub(crate) const GONE_GRACE: Duration = Duration::from_millis(500);

/// How often the list is looked at within [`GONE_GRACE`].
const GONE_POLL: Duration = Duration::from_millis(20);

/// `error`, or [`U3vError::Disconnected`] when it is one a missing device
/// also causes ([`U3vError::may_mean_gone`]) and `present` says the
/// operating system no longer lists the device — now, or within `grace`
/// (backlog UV-03). A transport that cannot tell leaves `error` as it is.
pub(crate) fn unless_gone(
    error: U3vError,
    present: impl Fn() -> Option<bool>,
    grace: Duration,
) -> U3vError {
    if !error.may_mean_gone() {
        return error;
    }
    let deadline = Instant::now() + grace;
    loop {
        match present() {
            Some(false) => {
                tracing::debug!(%error, "the device is no longer listed: it is gone");
                return U3vError::Disconnected;
            }
            Some(true) if Instant::now() < deadline => std::thread::sleep(GONE_POLL),
            Some(true) | None => return error,
        }
    }
}

/// A bulk IN endpoint's queue of transfers: submitted in order, completed in
/// order (USB bulk transfers on one endpoint complete in the order they were
/// queued).
pub trait BulkIn: Send {
    /// Queue a read of `len` bytes. It completes when `len` bytes have
    /// arrived, or earlier at a short packet.
    fn submit(&mut self, len: usize);

    /// Transfers queued and not yet returned by [`wait_next`](Self::wait_next).
    fn pending(&self) -> usize;

    /// The oldest queued transfer once it completes, waiting up to
    /// `timeout`; `None` if none completed in time — it stays queued — or if
    /// nothing is queued.
    fn wait_next(&mut self, timeout: Duration) -> Option<Completion>;

    /// Ask every queued transfer to end. Each still comes back from
    /// [`wait_next`](Self::wait_next), with whatever bytes arrived before it
    /// ended and a [`U3vError::Cancelled`] status unless it completed first.
    fn cancel_all(&mut self);

    /// Reset the endpoint, with nothing queued: halt it and clear the halt,
    /// so the device drops whatever it had queued on it and both sides
    /// restart the endpoint's data toggle. A stream does this before it is
    /// enabled, as aravis does (`arv_uv_device_reset_stream_endpoint`), and
    /// to recover from a stall.
    fn reset(&mut self) -> Result<(), U3vError>;

    /// [`UsbTransfer::is_present`] for the device this endpoint is on.
    fn is_present(&self) -> Option<bool> {
        None
    }
}

/// A transfer [`BulkIn::wait_next`] hands back.
#[derive(Debug)]
#[non_exhaustive]
pub struct Completion {
    /// The bytes that arrived, as many as were received: a cancelled or
    /// failed transfer may still carry some.
    pub data: Vec<u8>,
    /// How the transfer ended.
    pub status: Result<(), U3vError>,
}

impl Completion {
    /// A transfer that ended with `status`, carrying `data`.
    pub fn new(data: Vec<u8>, status: Result<(), U3vError>) -> Self {
        Self { data, status }
    }
}

/// [`UsbTransfer::bulk_in`]'s default: each queued read is performed, one at
/// a time, when its completion is asked for.
struct Unqueued<T: UsbTransfer> {
    transport: Arc<T>,
    endpoint: u8,
    requested: VecDeque<usize>,
    /// How many of the oldest requests were cancelled before being read.
    cancelled: usize,
}

impl<T: UsbTransfer + 'static> BulkIn for Unqueued<T> {
    fn submit(&mut self, len: usize) {
        self.requested.push_back(len);
    }

    fn pending(&self) -> usize {
        self.requested.len()
    }

    fn wait_next(&mut self, timeout: Duration) -> Option<Completion> {
        let len = *self.requested.front()?;
        if self.cancelled > 0 {
            self.cancelled -= 1;
            self.requested.pop_front();
            return Some(Completion::new(Vec::new(), Err(U3vError::Cancelled)));
        }
        let mut data = vec![0; len];
        match self.transport.bulk_read(self.endpoint, &mut data, timeout) {
            // Nothing within the wait: the request stays queued.
            Err(U3vError::Timeout) => None,
            Ok(n) => {
                self.requested.pop_front();
                data.truncate(n);
                Some(Completion::new(data, Ok(())))
            }
            Err(error) => {
                self.requested.pop_front();
                Some(Completion::new(Vec::new(), Err(error)))
            }
        }
    }

    fn cancel_all(&mut self) {
        self.cancelled = self.requested.len();
    }

    fn reset(&mut self) -> Result<(), U3vError> {
        // Nothing is queued on the device on our behalf between reads.
        Ok(())
    }

    fn is_present(&self) -> Option<bool> {
        self.transport.is_present()
    }
}

// ---------------------------------------------------------------------------
// Mock implementation for testing
// ---------------------------------------------------------------------------

use std::collections::HashMap;
use std::sync::Mutex;

/// What one endpoint of [`MockUsbTransfer`] answers, in order.
type ReadQueue = VecDeque<Result<Vec<u8>, U3vError>>;

/// In-memory mock of [`UsbTransfer`] for unit tests.
///
/// Pre-load expected read responses per endpoint, then execute protocol
/// logic. After the test, inspect captured writes to verify correctness.
///
/// # Thread safety
///
/// State lives behind a `Mutex`, so the `Send + Sync` that [`UsbTransfer`]
/// requires is derived rather than asserted. The mock used to hold `RefCell`
/// and claim the bounds with `unsafe impl`, which was unsound: `RefCell`'s
/// borrow flag is not atomic, so two threads sharing one mock could hold
/// overlapping `borrow_mut()`s and alias the same `&mut`.
pub struct MockUsbTransfer {
    /// Queued read responses per endpoint address.
    reads: Mutex<HashMap<u8, ReadQueue>>,
    /// Captured write payloads per endpoint address.
    writes: Mutex<HashMap<u8, Vec<Vec<u8>>>>,
    /// What [`UsbTransfer::is_present`] answers.
    present: Mutex<Option<bool>>,
}

impl MockUsbTransfer {
    /// Create an empty mock with no pre-loaded responses.
    pub fn new() -> Self {
        Self {
            reads: Mutex::new(HashMap::new()),
            writes: Mutex::new(HashMap::new()),
            present: Mutex::new(None),
        }
    }

    /// What [`UsbTransfer::is_present`] answers from now on: `Some(false)`
    /// for a device the operating system no longer lists. `None`, cannot
    /// tell, until set.
    pub fn set_present(&self, present: Option<bool>) {
        *self.present.lock().expect("mock presence poisoned") = present;
    }

    /// Enqueue a response that will be returned by the next `bulk_read`
    /// on the given `endpoint`.
    pub fn enqueue_read(&self, endpoint: u8, data: Vec<u8>) {
        self.enqueue(endpoint, Ok(data));
    }

    /// Enqueue an error the next `bulk_read` on `endpoint` fails with.
    pub fn enqueue_error(&self, endpoint: u8, error: U3vError) {
        self.enqueue(endpoint, Err(error));
    }

    fn enqueue(&self, endpoint: u8, read: Result<Vec<u8>, U3vError>) {
        self.reads
            .lock()
            .expect("mock read queue poisoned")
            .entry(endpoint)
            .or_default()
            .push_back(read);
    }

    /// Return all captured write payloads for the given `endpoint`.
    pub fn take_writes(&self, endpoint: u8) -> Vec<Vec<u8>> {
        self.writes
            .lock()
            .expect("mock write log poisoned")
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
            .lock()
            .expect("mock write log poisoned")
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
        let mut reads = self.reads.lock().expect("mock read queue poisoned");
        // Nothing queued is a device that does not answer: the host times
        // out.
        let data = reads
            .get_mut(&endpoint)
            .and_then(VecDeque::pop_front)
            .ok_or(U3vError::Timeout)??;
        let n = data.len().min(buf.len());
        buf[..n].copy_from_slice(&data[..n]);
        Ok(n)
    }

    fn is_present(&self) -> Option<bool> {
        *self.present.lock().expect("mock presence poisoned")
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
        assert!(matches!(err, U3vError::Timeout));
    }

    /// The default queue reads in submission order, keeps a request queued
    /// across a timeout, and hands back cancelled requests as such.
    #[test]
    fn the_default_queue_reads_one_request_at_a_time() {
        let mock = Arc::new(MockUsbTransfer::new());
        let mut queue = mock.bulk_in(0x82).unwrap();
        queue.submit(8);
        queue.submit(8);
        assert_eq!(queue.pending(), 2);
        assert!(queue.wait_next(Duration::from_millis(1)).is_none());
        assert_eq!(queue.pending(), 2, "a timeout keeps the request");

        mock.enqueue_read(0x82, vec![1, 2, 3]);
        let first = queue.wait_next(Duration::from_millis(1)).expect("done");
        assert_eq!(first.data, [1, 2, 3]);
        assert!(first.status.is_ok());

        mock.enqueue_error(0x82, U3vError::Stalled { endpoint: 0x82 });
        let failed = queue.wait_next(Duration::from_millis(1)).expect("done");
        assert!(matches!(
            failed.status,
            Err(U3vError::Stalled { endpoint: 0x82 })
        ));

        queue.submit(4);
        queue.cancel_all();
        mock.enqueue_read(0x82, vec![9; 4]);
        let cancelled = queue.wait_next(Duration::from_millis(1)).expect("done");
        assert!(matches!(cancelled.status, Err(U3vError::Cancelled)));
        assert_eq!(queue.pending(), 0);
        assert!(queue.wait_next(Duration::from_millis(1)).is_none());
    }
}
