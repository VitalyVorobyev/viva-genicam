//! USB3 Vision streaming protocol.
//!
//! U3V streaming uses USB bulk transfers with a leader/payload/trailer
//! structure. Unlike GigE Vision (GVSP over UDP), USB bulk transfers are
//! reliable and ordered — no packet reassembly, bitmap tracking, or resend
//! logic is needed.
//!
//! Each block arrives in transfers whose sizes the host wrote to the SIRM
//! ([`StreamLayout`]):
//!
//! ```text
//! ┌─────────┐   ┌──────────────────────────┐   ┌─────────┐   ┌─────────┐   ┌─────────┐
//! │ Leader  │ → │ count × payload transfer │ → │ final 1 │ → │ final 2 │ → │ Trailer │
//! └─────────┘   └──────────────────────────┘   └─────────┘   └─────────┘   └─────────┘
//! ```
//!
//! The leader and trailer layouts are the ones aravis (`ArvUvspLeader`,
//! `ArvUvspTrailer`) and Wireshark (`dissect_u3v_stream_leader`,
//! `dissect_u3v_stream_trailer`) agree on; the USB3 Vision standard itself is
//! not freely available. Every field is little-endian.
//!
//! # Transfers in flight (ADR-0032)
//!
//! [`U3vStream`] keeps several transfers queued on the endpoint
//! ([`DEFAULT_TRANSFERS_IN_FLIGHT`], as aravis keeps up to eight transfers'
//! worth): a device streaming at full rate has somewhere to put the next
//! transfer while the host is still handling the last one. It plans each
//! transfer's size from where the device is in its block — the leader, the
//! payload's transfers, the trailer — and checks every completion against
//! that plan. When they part — a block that ended early, a transfer that
//! failed — it cancels what is queued, reads what had already arrived, and
//! plans again from where the device really is: resynchronising on the next
//! leader.
//!
//! A transfer that fails — a stall, a fault — resets the endpoint and costs
//! the block in progress, not the stream (backlog UV-02). A disconnect ends
//! the stream with [`U3vError::Disconnected`]. Which errors a real camera
//! recovers from is open (backlog UV-05).

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;

use crate::U3vError;
use crate::usb::{BulkIn, Completion, GONE_GRACE, UsbTransfer, unless_gone};

/// U3V stream leader prefix: "U3VL" in little-endian.
const LEADER_PREFIX: u32 = 0x4C56_3355;
/// U3V stream trailer prefix: "U3VT" in little-endian.
const TRAILER_PREFIX: u32 = 0x5456_3355;

/// Prefix, reserved, size, block id, reserved, payload type: what every
/// leader carries.
const GENERIC_LEADER_SIZE: usize = 20;
/// An image leader, up to and including the x padding.
const IMAGE_LEADER_SIZE: usize = 50;
/// Prefix, reserved, size, block id, status, reserved, valid payload size.
const MIN_TRAILER_SIZE: usize = 28;

/// Payload type of an image (`ARV_UVSP_PAYLOAD_TYPE_IMAGE`).
pub const PAYLOAD_TYPE_IMAGE: u16 = 0x0001;
/// Payload type of an image followed by chunk data.
pub const PAYLOAD_TYPE_IMAGE_EXTENDED_CHUNK: u16 = 0x4001;
/// Payload type of chunk data alone.
pub const PAYLOAD_TYPE_CHUNK: u16 = 0x4000;

/// How long a block may go without a transfer before it is given up.
const STREAM_TIMEOUT: Duration = Duration::from_millis(5000);

/// Largest single payload transfer the host asks for: aravis's default
/// (`ARV_UV_STREAM_MAXIMUM_TRANSFER_SIZE_DEFAULT`, 1 MiB).
///
/// The device imposes only an alignment (SIRM Info); the size is the host's
/// to choose. The limit is the host's memory for transfers in flight: Linux
/// gives a process 16 MiB of usbfs memory by default (`usbfs_memory_mb`), and
/// [`DEFAULT_TRANSFERS_IN_FLIGHT`] of these fit in half of it.
pub const DEFAULT_MAX_TRANSFER_SIZE: u32 = 1 << 20;

/// Transfers a stream keeps queued: aravis's eight
/// (`ARV_UV_STREAM_N_MAXIMUM_SUBMITS`).
pub const DEFAULT_TRANSFERS_IN_FLIGHT: usize = 8;

/// How long cancelled transfers get to come back.
const DRAIN_TIMEOUT: Duration = Duration::from_millis(1000);

/// Transfer errors in a row, with no block finished between them, after
/// which the stream gives up: an endpoint that fails again at once after
/// every reset is not coming back.
const MAX_CONSECUTIVE_FAILURES: u32 = 8;

/// How the host reads one block: the sizes it writes to the SIRM, and reads
/// with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct StreamLayout {
    /// Bytes of payload the device sends in one block.
    pub payload_size: u64,
    /// Bytes the host reads for a leader.
    pub leader_size: u32,
    /// Bytes the host reads for a trailer.
    pub trailer_size: u32,
    /// Bytes in each full payload transfer.
    pub payload_transfer_size: u32,
    /// Number of full payload transfers.
    pub payload_transfer_count: u32,
    /// Bytes in the first final transfer, 0 when there is none.
    pub final_transfer1_size: u32,
    /// Bytes in the second final transfer, 0 when there is none.
    pub final_transfer2_size: u32,
}

impl StreamLayout {
    /// The layout for a block of `payload_size` bytes, every transfer a
    /// multiple of `alignment` and none larger than `max_transfer`.
    ///
    /// The USB3 Vision standard's own rules are not freely available, so
    /// this follows aravis (`arv_uv_stream_start_acquisition`): leader and
    /// trailer are the required sizes aligned up, the payload goes in
    /// transfers of up to `max_transfer` bytes, and the remainder in one
    /// final transfer aligned up. Unlike aravis, the payload transfer size is
    /// aligned *down* when the block is smaller than `max_transfer`, so no
    /// transfer breaks the alignment the device asked for; the remainder
    /// goes to the final transfer.
    pub fn new(
        payload_size: u64,
        required_leader_size: u32,
        required_trailer_size: u32,
        alignment: u32,
        max_transfer: u32,
    ) -> Result<Self, U3vError> {
        if alignment == 0 || alignment > max_transfer {
            return Err(U3vError::Device(
                "requires a transfer alignment larger than its transfers",
            ));
        }
        if payload_size == 0 {
            return Err(U3vError::Device("requires a zero-byte payload"));
        }
        let aligned_max = max_transfer / alignment * alignment;
        // A device that requires no leader or trailer size gets a whole
        // transfer, as aravis gives it.
        let framing = |required: u32| {
            if required == 0 {
                Ok(aligned_max)
            } else {
                align_up(required, alignment)
            }
        };

        let payload_transfer_size = if payload_size >= u64::from(aligned_max) {
            aligned_max
        } else {
            match (payload_size as u32) / alignment * alignment {
                0 => align_up(payload_size as u32, alignment)?,
                aligned => aligned,
            }
        };
        let count = payload_size / u64::from(payload_transfer_size);
        let payload_transfer_count = u32::try_from(count).map_err(|_| {
            U3vError::Device("requires more payload transfers than a SIRM can count")
        })?;
        let remainder = payload_size.saturating_sub(count * u64::from(payload_transfer_size));
        let final_transfer1_size = align_up(remainder as u32, alignment)?;

        Ok(Self {
            payload_size,
            leader_size: framing(required_leader_size)?,
            trailer_size: framing(required_trailer_size)?,
            payload_transfer_size,
            payload_transfer_count,
            final_transfer1_size,
            final_transfer2_size: 0,
        })
    }

    /// Bytes the payload transfers can hold.
    pub fn payload_capacity(&self) -> u64 {
        u64::from(self.payload_transfer_size) * u64::from(self.payload_transfer_count)
            + u64::from(self.final_transfer1_size)
            + u64::from(self.final_transfer2_size)
    }
}

fn align_up(value: u32, alignment: u32) -> Result<u32, U3vError> {
    value
        .div_ceil(alignment)
        .checked_mul(alignment)
        .ok_or(U3vError::Device(
            "requires an alignment that overflows a transfer size",
        ))
}

/// Parsed U3V stream leader.
#[derive(Debug, Clone)]
pub struct Leader {
    /// The block this leader opens; its trailer carries the same id.
    pub block_id: u64,
    /// Payload type ([`PAYLOAD_TYPE_IMAGE`], [`PAYLOAD_TYPE_IMAGE_EXTENDED_CHUNK`], …).
    pub payload_type: u16,
    /// Device timestamp, in ns: the GenCP timestamp (GenCP 1.3.1 §5.4.16),
    /// which aravis also reads as nanoseconds.
    pub timestamp: u64,
    /// PFNC pixel format code; 0 for a payload that is not an image.
    pub pixel_format: u32,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// X offset (ROI).
    pub x_offset: u32,
    /// Y offset (ROI).
    pub y_offset: u32,
    /// Horizontal padding bytes per line.
    pub x_padding: u16,
}

/// Parsed U3V stream trailer.
#[derive(Debug, Clone)]
pub struct Trailer {
    /// The block this trailer closes.
    pub block_id: u64,
    /// Status of the block (0 = success).
    pub status: u16,
    /// Bytes of payload the device actually sent.
    pub valid_payload_size: u64,
    /// Lines of the image the device sent, from an image trailer of 32 bytes
    /// or more; fewer than the leader's height when a variable-height frame
    /// ended early.
    pub size_y: Option<u32>,
}

/// A block as received from the U3V stream, before conversion to the
/// transport-agnostic `Frame` type.
#[derive(Debug)]
#[non_exhaustive]
pub struct RawFrame {
    /// Leader containing image metadata.
    pub leader: Leader,
    /// The payload bytes that arrived: up to the trailer's valid payload
    /// size when there is a trailer.
    pub payload: Bytes,
    /// The trailer, with status and block id; `None` for a block given up
    /// before it — a transfer failed, the next leader came first, or the
    /// device went quiet mid-block.
    pub trailer: Option<Trailer>,
}

/// What a stream has counted since it was opened.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct U3vStreamCounters {
    /// Transfers that completed, whatever they carried.
    pub transfers: u64,
    /// Transfers that were neither the leader, the payload nor the trailer
    /// due: the tail of a block whose start was missed, or payload past the
    /// layout's capacity.
    pub transfers_skipped: u64,
    /// Transfers that failed with a USB error the stream recovered from by
    /// resetting the endpoint (backlog UV-02).
    pub transfer_errors: u64,
    /// Blocks given up before their trailer.
    pub blocks_abandoned: u64,
}

/// Where the device is in its block: what the next transfer carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cursor {
    /// A leader opens the next block.
    Leader,
    /// The payload, this many bytes in.
    Payload(u64),
    /// The payload is complete, or the device ended it early.
    Trailer,
}

/// A block being received.
#[derive(Debug)]
struct Block {
    leader: Leader,
    payload: Vec<u8>,
    /// The device ended the payload with a short transfer.
    ended: bool,
    /// When a transfer last added to the block.
    last: Instant,
}

/// U3V stream receiver: reads blocks from a USB bulk endpoint, with
/// several transfers queued on it.
pub struct U3vStream {
    queue: Box<dyn BulkIn>,
    layout: StreamLayout,
    timeout: Duration,
    in_flight: usize,
    /// The role and size each queued transfer was planned with, oldest
    /// first.
    planned: VecDeque<(Cursor, usize)>,
    /// What the next transfer submitted is for.
    next: Cursor,
    block: Option<Block>,
    /// Blocks finished or given up, not yet handed out.
    ready: VecDeque<RawFrame>,
    counters: U3vStreamCounters,
    /// Transfer errors since the last block finished.
    failures: u32,
}

impl U3vStream {
    /// A receiver reading `queue` with `layout`, the transfer sizes written
    /// to the device's SIRM.
    pub fn new(queue: Box<dyn BulkIn>, layout: StreamLayout) -> Self {
        Self {
            queue,
            layout,
            timeout: STREAM_TIMEOUT,
            in_flight: DEFAULT_TRANSFERS_IN_FLIGHT,
            planned: VecDeque::new(),
            next: Cursor::Leader,
            block: None,
            ready: VecDeque::new(),
            counters: U3vStreamCounters::default(),
            failures: 0,
        }
    }

    /// A receiver on `transport`'s bulk IN `endpoint`.
    pub fn open<T: UsbTransfer + 'static>(
        transport: &Arc<T>,
        endpoint: u8,
        layout: StreamLayout,
    ) -> Result<Self, U3vError> {
        Ok(Self::new(transport.bulk_in(endpoint)?, layout))
    }

    /// The transfer layout this stream reads with.
    pub fn layout(&self) -> &StreamLayout {
        &self.layout
    }

    /// How long a block may go without a transfer before it is given up
    /// (default 5 s).
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// How many transfers to keep queued (default
    /// [`DEFAULT_TRANSFERS_IN_FLIGHT`], at least one).
    pub fn set_transfers_in_flight(&mut self, transfers: usize) {
        self.in_flight = transfers.max(1);
    }

    /// Transfers kept queued.
    pub fn transfers_in_flight(&self) -> usize {
        self.in_flight
    }

    /// What the stream has counted.
    pub fn counters(&self) -> U3vStreamCounters {
        self.counters
    }

    /// Receive the next block, waiting up to the stream timeout for it.
    pub fn next_frame(&mut self) -> Result<RawFrame, U3vError> {
        let deadline = Instant::now() + self.timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if let Some(frame) = self.next_frame_within(left)? {
                return Ok(frame);
            }
            if left.is_zero() {
                return Err(U3vError::Timeout);
            }
        }
    }

    /// The next block, finished or given up, or `Ok(None)` when no transfer
    /// completed within `idle`: the stream is waiting — in trigger mode, say
    /// — not broken, and a reader polling with this can stop between
    /// blocks.
    ///
    /// A block missing its trailer comes back with
    /// [`RawFrame::trailer`] `None`. A failed transfer resets the endpoint
    /// and the stream goes on; [`U3vError::Disconnected`], or transfers that
    /// keep failing after every reset, end it.
    pub fn next_frame_within(&mut self, idle: Duration) -> Result<Option<RawFrame>, U3vError> {
        loop {
            if let Some(frame) = self.ready.pop_front() {
                return Ok(Some(frame));
            }
            self.fill();
            let Some(completion) = self.queue.wait_next(idle) else {
                let stalled = self
                    .block
                    .as_ref()
                    .is_some_and(|block| block.last.elapsed() >= self.timeout);
                if stalled {
                    tracing::debug!("a block went quiet before its trailer; giving it up");
                    self.abandon();
                    continue;
                }
                return Ok(None);
            };
            let (planned, requested) = self
                .planned
                .pop_front()
                .unwrap_or((self.cursor(), completion.data.len()));
            self.complete(completion, requested, false)?;
            match self.planned.front() {
                // The device is no longer where the queued transfers were
                // planned for: plan them again from where it is.
                Some(&(next, _)) if next != self.cursor() => {
                    tracing::debug!(
                        ?planned,
                        expected = ?next,
                        found = ?self.cursor(),
                        "the stream left the planned transfers; resynchronising"
                    );
                    self.replan()?;
                }
                Some(_) => {}
                None => self.next = self.cursor(),
            }
        }
    }

    /// Queue transfers until `in_flight` are, sized for where the device
    /// will be.
    fn fill(&mut self) {
        while self.queue.pending() < self.in_flight {
            let (len, next) = self.slot(self.next);
            self.queue.submit(len);
            self.planned.push_back((self.next, len));
            self.next = next;
        }
    }

    /// The transfer the device sends at `cursor`: its size, and where the
    /// device is after it. Payload transfers end where the SIRM layout's do,
    /// so a plan started mid-transfer joins the device's boundaries again.
    fn slot(&self, cursor: Cursor) -> (usize, Cursor) {
        let layout = &self.layout;
        let capacity = layout.payload_capacity();
        match cursor {
            Cursor::Leader => (
                layout.leader_size as usize,
                if capacity == 0 {
                    Cursor::Trailer
                } else {
                    Cursor::Payload(0)
                },
            ),
            Cursor::Payload(offset) => {
                let size = u64::from(layout.payload_transfer_size);
                let full = size * u64::from(layout.payload_transfer_count);
                let final1 = full + u64::from(layout.final_transfer1_size);
                let end = if offset < full {
                    (offset / size + 1) * size
                } else if offset < final1 {
                    final1
                } else {
                    capacity
                };
                let next = if end >= capacity {
                    Cursor::Trailer
                } else {
                    Cursor::Payload(end)
                };
                ((end - offset) as usize, next)
            }
            Cursor::Trailer => (layout.trailer_size as usize, Cursor::Leader),
        }
    }

    /// Where the device is, by what has arrived.
    fn cursor(&self) -> Cursor {
        match &self.block {
            None => Cursor::Leader,
            Some(block) => {
                let received = block.payload.len() as u64;
                if block.ended || received >= self.layout.payload_capacity() {
                    Cursor::Trailer
                } else {
                    Cursor::Payload(received)
                }
            }
        }
    }

    /// Take in one completed transfer of `requested` bytes; `cancelling`
    /// while the stream itself is cancelling what it queued.
    fn complete(
        &mut self,
        completion: Completion,
        requested: usize,
        cancelling: bool,
    ) -> Result<(), U3vError> {
        match completion.status {
            // Cancelled by us before anything arrived: it never happened.
            Err(U3vError::Cancelled) if cancelling && completion.data.is_empty() => Ok(()),
            // A cancelled transfer still carries what arrived before.
            Ok(()) => {
                self.counters.transfers += 1;
                self.absorb(completion.data, requested);
                Ok(())
            }
            Err(U3vError::Cancelled) if cancelling => {
                self.counters.transfers += 1;
                self.absorb(completion.data, requested);
                Ok(())
            }
            // Ended by the host's USB stack, not by us: how macOS reports a
            // transfer it aborted (`kIOReturnAborted`) — a device reset or
            // removal, as the reset that follows will show.
            Err(U3vError::Cancelled) => {
                self.counters.transfers += 1;
                self.absorb(completion.data, requested);
                self.recover(U3vError::Cancelled)
            }
            Err(U3vError::Disconnected) => Err(U3vError::Disconnected),
            Err(error) => {
                self.counters.transfers += 1;
                self.recover(error)
            }
        }
    }

    /// Place one transfer's bytes: a leader opens a block, a trailer closes
    /// it, anything else is payload where payload is due.
    fn absorb(&mut self, data: Vec<u8>, requested: usize) {
        let leader = self.as_leader(&data);
        let trailer = self.as_trailer(&data);
        let capacity = self.layout.payload_capacity() as usize;
        match (&mut self.block, leader, trailer) {
            (_, Some(leader), _) => {
                if self.block.is_some() {
                    tracing::debug!("a leader came before the block's trailer");
                    self.abandon();
                }
                self.block = Some(Block {
                    leader,
                    payload: Vec::new(),
                    ended: capacity == 0,
                    last: Instant::now(),
                });
            }
            (Some(_), None, Some(trailer)) => self.finish(trailer),
            (Some(block), None, None) if !block.ended && block.payload.len() < capacity => {
                let room = capacity - block.payload.len();
                let ended = data.len() < requested || data.len() >= room;
                if data.len() > room {
                    self.counters.transfers_skipped += 1;
                }
                if block.payload.is_empty() && ended && data.len() <= room {
                    // The whole payload in one transfer: it is the frame,
                    // with no copy.
                    let mut data = data;
                    if data.capacity() > 2 * data.len() {
                        data.shrink_to_fit();
                    }
                    block.payload = data;
                } else {
                    if block.payload.capacity() == 0 {
                        block.payload.reserve_exact(capacity);
                    }
                    block
                        .payload
                        .extend_from_slice(&data[..data.len().min(room)]);
                }
                block.ended = ended;
                block.last = Instant::now();
            }
            _ => {
                tracing::debug!(
                    bytes = data.len(),
                    "skipping a transfer that is not what the stream expects"
                );
                self.counters.transfers_skipped += 1;
            }
        }
    }

    /// `data` as a leader: the prefix, a size field no longer than the
    /// transfer, and a transfer no longer than a leader's.
    fn as_leader(&self, data: &[u8]) -> Option<Leader> {
        let declared = framing_size(data, LEADER_PREFIX)?;
        (declared >= GENERIC_LEADER_SIZE && data.len() <= self.layout.leader_size as usize)
            .then(|| parse_leader(data).ok())
            .flatten()
    }

    /// `data` as a trailer, by the same rules.
    fn as_trailer(&self, data: &[u8]) -> Option<Trailer> {
        let declared = framing_size(data, TRAILER_PREFIX)?;
        (declared >= MIN_TRAILER_SIZE && data.len() <= self.layout.trailer_size as usize)
            .then(|| parse_trailer(data).ok())
            .flatten()
    }

    /// Close the block in progress with `trailer`.
    fn finish(&mut self, trailer: Trailer) {
        let Some(block) = self.block.take() else {
            return;
        };
        if trailer.block_id != block.leader.block_id {
            tracing::debug!(
                leader = block.leader.block_id,
                trailer = trailer.block_id,
                "a trailer closes another block than its leader opened"
            );
            self.block = Some(block);
            self.abandon();
            return;
        }
        let mut payload = block.payload;
        let valid = usize::try_from(trailer.valid_payload_size).unwrap_or(usize::MAX);
        payload.truncate(valid);
        self.failures = 0;
        self.ready.push_back(RawFrame {
            leader: block.leader,
            payload: Bytes::from(payload),
            trailer: Some(trailer),
        });
    }

    /// Give up the block in progress, handing out what arrived of it.
    fn abandon(&mut self) {
        if let Some(block) = self.block.take() {
            self.counters.blocks_abandoned += 1;
            self.ready.push_back(RawFrame {
                leader: block.leader,
                payload: Bytes::from(block.payload),
                trailer: None,
            });
        }
    }

    /// Cancel what is queued, take in what had already arrived, and plan
    /// again from where the device is.
    fn replan(&mut self) -> Result<(), U3vError> {
        self.queue.cancel_all();
        while self.queue.pending() > 0 {
            let Some(completion) = self.queue.wait_next(DRAIN_TIMEOUT) else {
                break;
            };
            let requested = self
                .planned
                .pop_front()
                .map_or(completion.data.len(), |(_, len)| len);
            let failed = matches!(
                completion.status,
                Err(ref e) if !matches!(e, U3vError::Cancelled)
            );
            self.complete(completion, requested, true)?;
            if failed {
                // `complete` recovered: the queue is empty and reset.
                return Ok(());
            }
        }
        self.planned.clear();
        self.next = self.cursor();
        Ok(())
    }

    /// A transfer failed: give up the block in progress, empty the queue,
    /// reset the endpoint and wait for the next leader — unless transfers
    /// keep failing.
    fn recover(&mut self, error: U3vError) -> Result<(), U3vError> {
        // An error that does not say whether the device is gone: one look at
        // whether it is still listed first (UV-03). It may still be, for a
        // moment, after an unplug; the reset below then fails, and that
        // failure waits for the device to leave.
        let error = unless_gone(error, || self.queue.is_present(), Duration::ZERO);
        if matches!(error, U3vError::Disconnected) {
            return Err(error);
        }
        self.counters.transfer_errors += 1;
        self.failures += 1;
        tracing::warn!(%error, "a stream transfer failed; resetting the endpoint");
        self.abandon();
        if self.failures > MAX_CONSECUTIVE_FAILURES {
            return Err(unless_gone(error, || self.queue.is_present(), GONE_GRACE));
        }
        self.queue.cancel_all();
        while self.queue.pending() > 0 {
            match self.queue.wait_next(DRAIN_TIMEOUT) {
                Some(Completion {
                    status: Err(U3vError::Disconnected),
                    ..
                }) => return Err(U3vError::Disconnected),
                Some(_) => {}
                None => break,
            }
        }
        self.planned.clear();
        self.queue
            .reset()
            .map_err(|error| unless_gone(error, || self.queue.is_present(), GONE_GRACE))?;
        self.next = Cursor::Leader;
        Ok(())
    }
}

/// The size field of a leader or trailer opening `data` with `prefix`, if
/// it is one: no more than the transfer holds.
fn framing_size(data: &[u8], prefix: u32) -> Option<usize> {
    let head = data.get(..8)?;
    if u32::from_le_bytes(head[..4].try_into().unwrap()) != prefix {
        return None;
    }
    let declared = usize::from(u16::from_le_bytes([head[6], head[7]]));
    (declared <= data.len()).then_some(declared)
}

// ---------------------------------------------------------------------------
// Wire format parsing
// ---------------------------------------------------------------------------

/// Parse a U3V stream leader from raw bytes.
///
/// Leader layout (little-endian):
/// ```text
/// [0..4]   prefix        "U3VL"
/// [4..6]   reserved
/// [6..8]   leader_size
/// [8..16]  block_id
/// [16..18] reserved
/// [18..20] payload_type
///          — image and image-extended-chunk payloads: —
/// [20..28] timestamp     (device ticks)
/// [28..32] pixel_format  (PFNC code)
/// [32..36] size_x
/// [36..40] size_y
/// [40..44] offset_x
/// [44..48] offset_y
/// [48..50] padding_x
/// [50..52] reserved
/// ```
pub fn parse_leader(buf: &[u8]) -> Result<Leader, U3vError> {
    if buf.len() < GENERIC_LEADER_SIZE {
        return Err(U3vError::Malformed {
            packet: "leader",
            problem: "is shorter than a generic leader",
        });
    }

    let prefix = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if prefix != LEADER_PREFIX {
        return Err(U3vError::Malformed {
            packet: "leader",
            problem: "lacks the U3VL prefix",
        });
    }

    let le32 = |at: usize| u32::from_le_bytes(buf[at..at + 4].try_into().unwrap());
    let le64 = |at: usize| u64::from_le_bytes(buf[at..at + 8].try_into().unwrap());
    let block_id = le64(8);
    let payload_type = u16::from_le_bytes([buf[18], buf[19]]);
    let mut leader = Leader {
        block_id,
        payload_type,
        timestamp: 0,
        pixel_format: 0,
        width: 0,
        height: 0,
        x_offset: 0,
        y_offset: 0,
        x_padding: 0,
    };

    if matches!(
        payload_type,
        PAYLOAD_TYPE_IMAGE | PAYLOAD_TYPE_IMAGE_EXTENDED_CHUNK
    ) {
        if buf.len() < IMAGE_LEADER_SIZE {
            return Err(U3vError::Malformed {
                packet: "image leader",
                problem: "is shorter than an image leader",
            });
        }
        leader.timestamp = le64(20);
        leader.pixel_format = le32(28);
        leader.width = le32(32);
        leader.height = le32(36);
        leader.x_offset = le32(40);
        leader.y_offset = le32(44);
        leader.x_padding = u16::from_le_bytes([buf[48], buf[49]]);
    } else if buf.len() >= 28 {
        // A chunk payload's leader carries its timestamp at the same place.
        leader.timestamp = le64(20);
    }

    Ok(leader)
}

/// Parse a U3V stream trailer from raw bytes.
///
/// Trailer layout (little-endian):
/// ```text
/// [0..4]   prefix              "U3VT"
/// [4..6]   reserved
/// [6..8]   trailer_size
/// [8..16]  block_id
/// [16..18] status
/// [18..20] reserved
/// [20..28] valid_payload_size
/// [28..32] size_y              (image payloads)
/// ```
pub fn parse_trailer(buf: &[u8]) -> Result<Trailer, U3vError> {
    if buf.len() < MIN_TRAILER_SIZE {
        return Err(U3vError::Malformed {
            packet: "trailer",
            problem: "is shorter than a trailer",
        });
    }

    let prefix = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if prefix != TRAILER_PREFIX {
        return Err(U3vError::Malformed {
            packet: "trailer",
            problem: "lacks the U3VT prefix",
        });
    }

    Ok(Trailer {
        block_id: u64::from_le_bytes(buf[8..16].try_into().unwrap()),
        status: u16::from_le_bytes([buf[16], buf[17]]),
        valid_payload_size: u64::from_le_bytes(buf[20..28].try_into().unwrap()),
        size_y: buf
            .get(28..32)
            .map(|y| u32::from_le_bytes(y.try_into().unwrap())),
    })
}

/// A chunk's tag: its ID, then the length of the data before it.
const CHUNK_TAG_LEN: usize = 8;

/// The chunks of a USB3 Vision chunk payload, as `(chunk ID, offset,
/// length)` in payload order.
///
/// The layout is GigE Vision's — each chunk its data followed by an 8-byte
/// tag, so the payload is read from its end — with the tag's ID and length
/// little-endian, as aravis reads a USB3 Vision buffer's chunks
/// (`arvuvstream.c` sets `chunk_endianness` to little-endian; corroboration
/// only, the USB3 Vision standard is not available to us). With an image
/// payload the image is the first chunk.
///
/// Remote input: a tag that claims more than precedes it, or a front too
/// short for a tag, is an error.
pub fn parse_chunks(payload: &[u8]) -> Result<Vec<(u32, usize, usize)>, U3vError> {
    let mut chunks = Vec::new();
    let mut end = payload.len();
    while end > 0 {
        let tag_offset = end.checked_sub(CHUNK_TAG_LEN).ok_or(U3vError::Malformed {
            packet: "chunk layout",
            problem: "ends in a tag shorter than eight bytes",
        })?;
        let tag = &payload[tag_offset..end];
        let id = u32::from_le_bytes(tag[..4].try_into().unwrap());
        let length = u32::from_le_bytes(tag[4..].try_into().unwrap()) as usize;
        let start = tag_offset.checked_sub(length).ok_or(U3vError::Malformed {
            packet: "chunk layout",
            problem: "has a tag claiming more data than precedes it",
        })?;
        chunks.push((id, start, length));
        end = start;
    }
    chunks.reverse();
    Ok(chunks)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usb::MockUsbTransfer;

    const EP: u8 = 0x82;

    /// A camera with no frame to send — trigger mode, waiting for a trigger —
    /// times out on every read.
    struct Idle;

    impl UsbTransfer for Idle {
        fn bulk_write(&self, _: u8, data: &[u8], _: Duration) -> Result<usize, U3vError> {
            Ok(data.len())
        }
        fn bulk_read(&self, _: u8, _: &mut [u8], _: Duration) -> Result<usize, U3vError> {
            Err(U3vError::Timeout)
        }
    }

    fn layout(payload_size: u64, transfer: u32) -> StreamLayout {
        StreamLayout::new(payload_size, 52, 32, 4, transfer).unwrap()
    }

    fn stream(mock: &Arc<MockUsbTransfer>, layout: StreamLayout) -> U3vStream {
        U3vStream::open(mock, EP, layout).unwrap()
    }

    /// An idle stream is waiting, not broken, so a reader can poll it and
    /// stop between frames (Codex review of #179).
    #[test]
    fn an_idle_stream_is_waiting_not_broken() {
        let mut stream = U3vStream::open(&Arc::new(Idle), EP, layout(4096, 4096)).unwrap();
        let idle = stream.next_frame_within(Duration::from_millis(10));
        assert!(matches!(idle, Ok(None)), "{idle:?}");
    }

    fn build_leader(block_id: u64, width: u32, height: u32, pixel_format: u32) -> Vec<u8> {
        let mut buf = vec![0u8; 52];
        buf[0..4].copy_from_slice(&LEADER_PREFIX.to_le_bytes());
        buf[6..8].copy_from_slice(&52u16.to_le_bytes());
        buf[8..16].copy_from_slice(&block_id.to_le_bytes());
        buf[18..20].copy_from_slice(&PAYLOAD_TYPE_IMAGE.to_le_bytes());
        buf[20..28].copy_from_slice(&(block_id * 1000).to_le_bytes());
        buf[28..32].copy_from_slice(&pixel_format.to_le_bytes());
        buf[32..36].copy_from_slice(&width.to_le_bytes());
        buf[36..40].copy_from_slice(&height.to_le_bytes());
        buf
    }

    fn build_trailer(block_id: u64, status: u16, valid_payload_size: u64) -> Vec<u8> {
        let mut buf = vec![0u8; 32];
        buf[0..4].copy_from_slice(&TRAILER_PREFIX.to_le_bytes());
        buf[6..8].copy_from_slice(&32u16.to_le_bytes());
        buf[8..16].copy_from_slice(&block_id.to_le_bytes());
        buf[16..18].copy_from_slice(&status.to_le_bytes());
        buf[20..28].copy_from_slice(&valid_payload_size.to_le_bytes());
        buf[28..32].copy_from_slice(&7u32.to_le_bytes());
        buf
    }

    #[test]
    fn parse_leader_bad_prefix() {
        let mut data = build_leader(1, 640, 480, 0);
        data[0] = 0xFF;
        assert!(parse_leader(&data).is_err());
    }

    #[test]
    fn parse_leader_too_short() {
        assert!(parse_leader(&[0u8; 10]).is_err());
        // A leader that says "image" must carry the image fields.
        assert!(parse_leader(&build_leader(1, 8, 8, 0)[..40]).is_err());
    }

    #[test]
    fn parse_trailer_bad_prefix() {
        let mut data = build_trailer(0, 0, 0);
        data[0] = 0xFF;
        assert!(parse_trailer(&data).is_err());
    }

    #[test]
    fn parse_trailer_too_short() {
        assert!(parse_trailer(&[0u8; 8]).is_err());
    }

    /// An image trailer of 32 bytes carries Size Y; a 28-byte one does not.
    #[test]
    fn a_trailer_carries_size_y_when_long_enough() {
        let trailer = build_trailer(3, 0, 64);
        assert_eq!(parse_trailer(&trailer).unwrap().size_y, Some(7));
        assert_eq!(parse_trailer(&trailer[..28]).unwrap().size_y, None);
    }

    /// Chunk tags are little-endian, read from the end; the image chunk is
    /// first.
    #[test]
    fn chunks_are_read_back_from_little_endian_tags() {
        #[rustfmt::skip]
        let payload = [
            0xA0, 0xA1, 0xA2, 0xA3,
            0x01, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00,
            0x2A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x0F, 0x00, 0x08, 0x05, 0x08, 0x00, 0x00, 0x00,
        ];
        assert_eq!(
            parse_chunks(&payload).unwrap(),
            [(1, 0, 4), (0x0508_000F, 12, 8)]
        );
        assert!(parse_chunks(&payload[1..]).is_err());
        assert!(parse_chunks(&payload[..5]).is_err());
    }

    #[test]
    fn a_layout_covers_the_payload_in_aligned_transfers() {
        // Smaller than one transfer: one transfer, aligned down, and the rest
        // in the final transfer, aligned up.
        let small = StreamLayout::new(10_001, 52, 32, 8, 1 << 20).unwrap();
        assert_eq!(small.leader_size, 56);
        assert_eq!(small.trailer_size, 32);
        assert_eq!(
            (small.payload_transfer_size, small.payload_transfer_count),
            (10_000, 1)
        );
        assert_eq!(small.final_transfer1_size, 8);
        assert_eq!(small.final_transfer2_size, 0);

        // Larger: whole transfers of the maximum, then the remainder.
        let large = StreamLayout::new(3_000_000, 52, 32, 1024, 1 << 20).unwrap();
        assert_eq!(
            (large.payload_transfer_size, large.payload_transfer_count),
            (1 << 20, 2)
        );
        assert_eq!(large.final_transfer1_size, 903_168); // 902 848 aligned up
        assert!(large.payload_capacity() >= 3_000_000);

        // A payload below the alignment still gets one transfer.
        let tiny = StreamLayout::new(3, 52, 32, 8, 1 << 20).unwrap();
        assert_eq!(
            (tiny.payload_transfer_size, tiny.payload_transfer_count),
            (8, 0)
        );
        assert_eq!(tiny.final_transfer1_size, 8);

        assert!(StreamLayout::new(0, 52, 32, 8, 1 << 20).is_err());
        assert!(StreamLayout::new(100, 52, 32, 0, 1 << 20).is_err());
    }

    #[test]
    fn stream_next_frame() {
        let mock = Arc::new(MockUsbTransfer::new());
        let payload_size = 320 * 240;

        mock.enqueue_read(EP, build_leader(7, 320, 240, 0x0108_0001));
        mock.enqueue_read(EP, vec![0x80; payload_size]);
        mock.enqueue_read(EP, build_trailer(7, 0, payload_size as u64));

        let mut stream = stream(&mock, layout(payload_size as u64, 1 << 20));
        let frame = stream.next_frame().unwrap();

        assert_eq!(frame.leader.block_id, 7);
        assert_eq!((frame.leader.width, frame.leader.height), (320, 240));
        assert_eq!(frame.leader.timestamp, 7000);
        assert_eq!(frame.payload.len(), payload_size);
        assert_eq!(frame.trailer.expect("a trailer").status, 0);
    }

    /// Each transfer is read at the size the layout gives it.
    #[test]
    fn the_payload_is_read_in_the_layouts_transfers() {
        let mock = Arc::new(MockUsbTransfer::new());
        let stream_layout = layout(1000, 256); // 3 × 256, final 232
        assert_eq!(stream_layout.payload_transfer_count, 3);
        assert_eq!(stream_layout.final_transfer1_size, 232);

        mock.enqueue_read(EP, build_leader(1, 25, 40, 0x0108_0001));
        for fill in 0..3u8 {
            mock.enqueue_read(EP, vec![fill; 256]);
        }
        mock.enqueue_read(EP, vec![3; 232]);
        mock.enqueue_read(EP, build_trailer(1, 0, 1000));

        let mut stream = stream(&mock, stream_layout);
        let frame = stream.next_frame().unwrap();
        assert_eq!(frame.payload.len(), 1000);
        assert!(frame.payload[..256].iter().all(|&b| b == 0));
        assert!(frame.payload[768..].iter().all(|&b| b == 3));
    }

    /// A short transfer ends the payload; the trailer says how much is valid.
    #[test]
    fn a_short_payload_ends_at_the_trailer() {
        let mock = Arc::new(MockUsbTransfer::new());
        mock.enqueue_read(EP, build_leader(2, 25, 40, 0x0108_0001));
        mock.enqueue_read(EP, vec![9; 100]);
        mock.enqueue_read(EP, build_trailer(2, 0xA100, 100));

        let mut stream = stream(&mock, layout(1000, 256));
        let frame = stream.next_frame().unwrap();
        assert_eq!(frame.payload.len(), 100);
        assert_eq!(frame.trailer.expect("a trailer").status, 0xA100);
    }

    #[test]
    fn a_stray_transfer_before_the_leader_is_skipped() {
        let mock = Arc::new(MockUsbTransfer::new());
        mock.enqueue_read(EP, vec![0xEE; 64]);
        mock.enqueue_read(EP, build_leader(3, 8, 8, 0x0108_0001));
        mock.enqueue_read(EP, vec![1; 64]);
        mock.enqueue_read(EP, build_trailer(3, 0, 64));

        let mut stream = stream(&mock, layout(64, 256));
        assert_eq!(stream.next_frame().unwrap().leader.block_id, 3);
        assert_eq!(stream.counters().transfers_skipped, 1);
    }

    /// A trailer for another block closes nothing: the block comes back
    /// without one, and the stream goes on.
    #[test]
    fn a_trailer_for_another_block_gives_the_block_up() {
        let mock = Arc::new(MockUsbTransfer::new());
        mock.enqueue_read(EP, build_leader(4, 8, 8, 0x0108_0001));
        mock.enqueue_read(EP, vec![1; 64]);
        mock.enqueue_read(EP, build_trailer(5, 0, 64));
        mock.enqueue_read(EP, build_leader(6, 8, 8, 0x0108_0001));
        mock.enqueue_read(EP, vec![2; 64]);
        mock.enqueue_read(EP, build_trailer(6, 0, 64));

        let mut stream = stream(&mock, layout(64, 256));
        let given_up = stream.next_frame().unwrap();
        assert_eq!(given_up.leader.block_id, 4);
        assert!(given_up.trailer.is_none());
        let next = stream.next_frame().unwrap();
        assert_eq!(next.leader.block_id, 6);
        assert!(next.trailer.is_some());
    }

    /// UV-02: a stall costs the block in progress and resets the endpoint;
    /// the stream goes on with the next leader. A disconnect ends it.
    #[test]
    fn a_stall_costs_a_block_and_a_disconnect_ends_the_stream() {
        let mock = Arc::new(MockUsbTransfer::new());
        mock.enqueue_read(EP, build_leader(1, 8, 8, 0x0108_0001));
        mock.enqueue_error(EP, U3vError::Stalled { endpoint: EP });
        mock.enqueue_read(EP, build_leader(2, 8, 8, 0x0108_0001));
        mock.enqueue_read(EP, vec![2; 64]);
        mock.enqueue_read(EP, build_trailer(2, 0, 64));
        mock.enqueue_error(EP, U3vError::Disconnected);

        let mut stream = stream(&mock, layout(64, 256));
        let lost = stream.next_frame().unwrap();
        assert_eq!(lost.leader.block_id, 1);
        assert!(lost.trailer.is_none());
        let next = stream.next_frame().unwrap();
        assert_eq!(next.leader.block_id, 2);
        assert_eq!(next.payload.len(), 64);
        let counters = stream.counters();
        assert_eq!(
            (counters.transfer_errors, counters.blocks_abandoned),
            (1, 1)
        );
        assert!(matches!(stream.next_frame(), Err(U3vError::Disconnected)));
    }

    /// A transfer the host's stack ended — not the stream — is a failure to
    /// recover from, not a transfer that never happened: on macOS an unplug
    /// or a device reset ends transfers so, and retrying them at once would
    /// spin.
    #[test]
    fn a_transfer_the_host_aborted_is_recovered_from() {
        let mock = Arc::new(MockUsbTransfer::new());
        mock.enqueue_read(EP, build_leader(1, 8, 8, 0x0108_0001));
        mock.enqueue_error(EP, U3vError::Cancelled);
        mock.enqueue_read(EP, build_leader(2, 8, 8, 0x0108_0001));
        mock.enqueue_read(EP, vec![2; 64]);
        mock.enqueue_read(EP, build_trailer(2, 0, 64));

        let mut stream = stream(&mock, layout(64, 256));
        assert!(stream.next_frame().unwrap().trailer.is_none());
        assert_eq!(stream.next_frame().unwrap().leader.block_id, 2);
        assert_eq!(stream.counters().transfer_errors, 1);
    }

    /// What macOS reported when an mvBlueFOX3-2004G's cable was pulled
    /// mid-stream: `kIOReturnNotResponding`, which `nusb` passes on as an
    /// unknown error, not as a disconnect.
    fn not_responding() -> U3vError {
        U3vError::usb("unknown (error 0xe00002ed)")
    }

    /// UV-03 on macOS: a transfer fails with an error that does not say the
    /// device is gone, and the operating system no longer lists the device:
    /// the stream ends with a disconnect, not that error.
    #[test]
    fn an_unknown_failure_on_a_device_no_longer_listed_is_a_disconnect() {
        let mock = Arc::new(MockUsbTransfer::new());
        mock.enqueue_read(EP, build_leader(1, 8, 8, 0x0108_0001));
        mock.enqueue_error(EP, not_responding());
        mock.set_present(Some(false));

        let mut stream = stream(&mock, layout(64, 256));
        let result = stream.next_frame();
        assert!(matches!(result, Err(U3vError::Disconnected)), "{result:?}");
    }

    /// The same error on a device still listed is a failure like any
    /// other: it costs the block, and the stream goes on.
    #[test]
    fn an_unknown_failure_on_a_device_still_listed_is_recovered_from() {
        let mock = Arc::new(MockUsbTransfer::new());
        mock.enqueue_read(EP, build_leader(1, 8, 8, 0x0108_0001));
        mock.enqueue_error(EP, not_responding());
        mock.enqueue_read(EP, build_leader(2, 8, 8, 0x0108_0001));
        mock.enqueue_read(EP, vec![2; 64]);
        mock.enqueue_read(EP, build_trailer(2, 0, 64));
        mock.set_present(Some(true));

        let mut stream = stream(&mock, layout(64, 256));
        assert!(stream.next_frame().unwrap().trailer.is_none());
        let next = stream.next_frame().unwrap();
        assert_eq!(next.leader.block_id, 2);
        assert!(next.trailer.is_some());
        assert_eq!(stream.counters().transfer_errors, 1);
    }

    /// An endpoint that fails after every reset ends the stream.
    #[test]
    fn transfers_that_keep_failing_end_the_stream() {
        let mock = Arc::new(MockUsbTransfer::new());
        for _ in 0..=MAX_CONSECUTIVE_FAILURES {
            mock.enqueue_error(EP, U3vError::Stalled { endpoint: EP });
        }
        let mut stream = stream(&mock, layout(64, 256));
        let mut result = stream.next_frame_within(Duration::from_millis(1));
        for _ in 0..MAX_CONSECUTIVE_FAILURES {
            if result.is_err() {
                break;
            }
            result = stream.next_frame_within(Duration::from_millis(1));
        }
        assert!(
            matches!(result, Err(U3vError::Stalled { endpoint: EP })),
            "{result:?}"
        );
    }
}
