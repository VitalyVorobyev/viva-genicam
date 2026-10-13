//! Fake USB transport implementing [`UsbTransfer`] via an in-memory register map.
//!
//! The control endpoint speaks GenCP as GenCP 1.2 defines it — the `U3VC`
//! prefix both ways, RequestAck at bit 14, READMEM `0x0800` and WRITEMEM
//! `0x0802`, every field little-endian — and refuses what a device refuses.
//! Its constants are its own, not `viva-u3v`'s (ADR-0019, backlog TC-27).
//!
//! The stream endpoint behaves as a USB bulk IN endpoint does: the device
//! sends a block only while the host has set StreamEnable and started
//! acquisition, as transfers of the sizes the host wrote to the SIRM, cut
//! into [`MAX_PACKET`]-byte packets. A host transfer collects packets until
//! it is full or a short packet ends it; a device transfer that is a whole
//! number of packets ends no host transfer, so a host reading with other
//! sizes than it wrote sees the device's transfers run together, as on USB.
//!
//! What a test can make the device do, and read back from it:
//!
//! - [`require_queued_transfers`](FakeU3vTransport::require_queued_transfers):
//!   a device streaming at full rate overruns — drops the rest of a block's
//!   payload and says so in its trailer — unless the host has that many
//!   transfers queued; [`max_queued_transfers`](FakeU3vTransport::max_queued_transfers)
//!   is how many it saw at once (backlog UV-01).
//! - [`stall_stream_after`](FakeU3vTransport::stall_stream_after): the stream
//!   endpoint halts mid-block, and stays halted until the host resets it
//!   (UV-02); [`log`](FakeU3vTransport::log) records each reset among the
//!   register writes.
//! - [`unplug`](FakeU3vTransport::unplug): every transfer fails with
//!   [`U3vError::Disconnected`] from then on (UV-03), as Linux reports it.
//!   [`unplug_as_macos_does`](FakeU3vTransport::unplug_as_macos_does): the
//!   transfers fail with the unknown IOKit codes macOS gave on a real
//!   unplug instead, and the device stays listed a moment longer — only
//!   asking whether it is still listed tells.
//! - Events on the event endpoint, GenCP's `EVENT_CMD` (GenCP 1.3.1
//!   Table 17), one per frame while the EIRM's EventEnable and the
//!   ExposureEnd notification are on.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::{BufMut, BytesMut};
use viva_gencp::StatusCode;
use viva_u3v::{BulkIn, Completion, U3vError, UsbTransfer};

use crate::registers::{
    ABRM_TIMESTAMP_LATCH, CHUNK_ID_FRAME_ID, CHUNK_ID_IMAGE, CHUNK_ID_TIMESTAMP,
    EVENT_ID_EXPOSURE_END, LEADER_SIZE, REG_ACQ_START, REG_ACQ_STOP, RegisterMap, SIRM_CONTROL,
    TRAILER_SIZE,
};

/// "U3VC", little-endian: the prefix of every command and acknowledge.
const CONTROL_PREFIX: [u8; 4] = *b"U3VC";
/// "U3VE": the prefix of an event command (Wireshark `U3V_EVENT_PREFIX`).
const EVENT_PREFIX: [u8; 4] = *b"U3VE";
/// Prefix plus common command data.
const HEADER_SIZE: usize = 12;
// GenCP's flag and command ids (Tables 4 and 7), imported as vocabulary
// (ADR-0030).
use viva_gencp::ccd_flags::REQUEST_ACK;
use viva_gencp::command_id::{EVENT_CMD, READMEM_CMD, WRITEMEM_CMD};

use crate::{EVENT_EP_IN as EVENT_EP, STREAM_EP_IN as STREAM_EP};

/// "U3VL" and "U3VT", little-endian.
const LEADER_PREFIX: [u8; 4] = *b"U3VL";
const TRAILER_PREFIX: [u8; 4] = *b"U3VT";

/// A SuperSpeed bulk endpoint's packet size (USB 3.2 §9.6.6).
pub const MAX_PACKET: usize = 1024;

/// The trailer status of a block whose payload the device dropped because
/// the host was not reading: USB3 Vision's `DATA_OVERRUN`, as aravis names
/// `0xA101`.
const DATA_OVERRUN: u16 = 0xA101;

/// Longest a stream read blocks before reporting no data, standing in for
/// the host's timeout so an idle reader does not spin.
const IDLE_READ: Duration = Duration::from_millis(20);

/// What the device saw, in order: the register writes it accepted and the
/// endpoint resets the host made.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FakeUsbEvent {
    /// A WRITEMEM the device accepted.
    Write {
        /// Where.
        address: u64,
        /// What.
        data: Vec<u8>,
    },
    /// The host halted and cleared an endpoint.
    EndpointReset {
        /// Which.
        endpoint: u8,
    },
}

/// Fake USB transport backed by an in-memory register map.
///
/// Implements [`UsbTransfer`] by executing bulk writes as GenCP commands and
/// queuing their acknowledges. Stream endpoint reads return synthetic blocks.
pub struct FakeU3vTransport {
    state: Mutex<TransportState>,
}

struct TransportState {
    registers: RegisterMap,
    /// The acknowledge for the last command, until the host reads it.
    pending_ack: Option<Vec<u8>>,
    /// Whether AcquisitionStart has run without an AcquisitionStop since.
    acquiring: bool,
    /// The block being sent, once its leader has gone.
    block: Option<Block>,
    next_block_id: u64,
    /// `(address, length)` of every register read the device was asked for,
    /// served or refused, in order.
    reads: Vec<(u64, usize)>,
    /// `(address, bytes)` of every write the device accepted, in order.
    writes: Vec<(u64, Vec<u8>)>,
    log: Vec<FakeUsbEvent>,
    /// When the device's clock read 0.
    clock: Instant,
    /// Transfers the host has queued on the stream endpoint now, and the
    /// most it ever had.
    host_queued: usize,
    max_host_queued: usize,
    /// Below this many queued transfers a block's payload overruns.
    required_queued: usize,
    /// Payload packets to send before the stream endpoint halts.
    stall_after: Option<u64>,
    /// The stream endpoint is halted until the host resets it.
    halted: bool,
    resets: Vec<u8>,
    unplugged: bool,
    /// An unplug that reports what macOS reported: unknown IOKit codes,
    /// not a disconnect.
    unplug_codes: bool,
    /// Until when an unplugged device is still listed.
    listed_until: Option<Instant>,
    /// Event commands waiting for the host.
    events: VecDeque<Vec<u8>>,
    event_request_id: u16,
    /// When the last block started: blocks start at `AcquisitionFrameRate`.
    last_block_start: Option<Instant>,
}

/// One block on its way out: the device transfers it is sent in, and how
/// far it has got.
struct Block {
    id: u64,
    timestamp: u64,
    /// The device transfers still to send, the one in progress first.
    transfers: VecDeque<DeviceTransfer>,
    /// Payload bytes sent.
    sent: usize,
    /// The device gave up the payload: the host was not reading.
    overrun: bool,
    height: u32,
}

/// A device-side transfer and how much of it has gone.
struct DeviceTransfer {
    kind: Part,
    data: Vec<u8>,
    offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Leader,
    Payload,
    /// Built when it is due, once the payload's fate is known.
    Trailer,
}

impl FakeU3vTransport {
    /// Create a new fake transport with the given image dimensions.
    pub fn new(width: u32, height: u32, pixel_format: u32) -> Self {
        Self {
            state: Mutex::new(TransportState {
                registers: RegisterMap::new(width, height, pixel_format),
                pending_ack: None,
                acquiring: false,
                block: None,
                // The camera this models starts at 0 (see `reset`).
                next_block_id: 0,
                reads: Vec::new(),
                writes: Vec::new(),
                log: Vec::new(),
                clock: Instant::now(),
                host_queued: 0,
                max_host_queued: 0,
                required_queued: 0,
                stall_after: None,
                halted: false,
                resets: Vec::new(),
                unplugged: false,
                unplug_codes: false,
                listed_until: None,
                events: VecDeque::new(),
                event_request_id: 0,
                last_block_start: None,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TransportState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `(address, length)` of every READMEM received so far, including the
    /// ones refused for touching a write-only register.
    ///
    /// The device's own record, so a test can tell a read that never happened
    /// from one whose failure the client swallowed.
    pub fn reads(&self) -> Vec<(u64, usize)> {
        self.lock().reads.clone()
    }

    /// Forget the reads recorded so far, e.g. the XML fetch at open.
    pub fn clear_reads(&self) {
        self.lock().reads.clear();
    }

    /// Whether any recorded read touched the 4-byte register at `addr`.
    pub fn was_read(&self, addr: u64) -> bool {
        self.lock()
            .reads
            .iter()
            .any(|&(a, len)| a < addr + 4 && addr < a.saturating_add(len as u64))
    }

    /// `(address, bytes)` of every write the device accepted, in order.
    pub fn writes(&self) -> Vec<(u64, Vec<u8>)> {
        self.lock().writes.clone()
    }

    /// The writes the device accepted and the endpoint resets the host made,
    /// in the order they happened.
    pub fn log(&self) -> Vec<FakeUsbEvent> {
        self.lock().log.clone()
    }

    /// The device's register bytes at `[addr, addr + len)`, as it holds them.
    pub fn peek(&self, addr: u64, len: usize) -> Vec<u8> {
        self.lock().registers.read(addr, len)
    }

    /// Whether the host has StreamEnable set.
    pub fn stream_enabled(&self) -> bool {
        self.lock().registers.stream_enabled()
    }

    /// Whether the host has EventEnable set in the EIRM.
    pub fn events_enabled(&self) -> bool {
        self.lock().registers.events_enabled()
    }

    /// Whether acquisition is running.
    pub fn acquiring(&self) -> bool {
        self.lock().acquiring
    }

    /// Make the device overrun — drop the rest of a block's payload, and
    /// end it with a `DATA_OVERRUN` trailer — whenever it has payload to send
    /// and the host has fewer than `transfers` queued on the stream
    /// endpoint: a camera streaming at full rate, which cannot wait for a
    /// host that reads one transfer at a time.
    pub fn require_queued_transfers(&self, transfers: usize) {
        self.lock().required_queued = transfers;
    }

    /// The most transfers the host has had queued on the stream endpoint at
    /// once. A read through `bulk_read` counts as one.
    pub fn max_queued_transfers(&self) -> usize {
        self.lock().max_host_queued
    }

    /// Halt the stream endpoint after `packets` more payload packets: the
    /// transfer in progress fails with a stall, the block is lost, and every
    /// transfer stalls until the host resets the endpoint.
    pub fn stall_stream_after(&self, packets: u64) {
        self.lock().stall_after = Some(packets);
    }

    /// The endpoints the host has reset, in order.
    pub fn endpoint_resets(&self) -> Vec<u8> {
        self.lock().resets.clone()
    }

    /// Unplug the camera: every transfer fails with
    /// [`U3vError::Disconnected`] from now on, and the device is no longer
    /// listed.
    pub fn unplug(&self) {
        self.lock().unplugged = true;
    }

    /// Unplug the camera as macOS reported it when an mvBlueFOX3-2004G's
    /// cable was pulled mid-stream: a bulk IN transfer — and the endpoint
    /// reset — fail with `kIOReturnNotResponding` (`0xe00002ed`), a bulk
    /// OUT transfer with `kIOReturnNotReady` (`0xe00002d8`), both unknown
    /// errors to `nusb`, never [`U3vError::Disconnected`]. The device stays
    /// listed for `listed_for` after the unplug, as it can while the host
    /// notices.
    pub fn unplug_as_macos_does(&self, listed_for: Duration) {
        let mut state = self.lock();
        state.unplugged = true;
        state.unplug_codes = true;
        state.listed_until = Some(Instant::now() + listed_for);
    }

    /// Report `bits` in the SBRM's current speed register:
    /// [`HIGH_SPEED`](crate::registers::HIGH_SPEED) for a camera on a USB 2
    /// link.
    pub fn set_current_speed(&self, bits: u32) {
        self.lock().registers.set_current_speed(bits);
    }

    /// The device's clock, in ns: what its leaders, chunks and events are
    /// stamped with, and what a timestamp latch reads.
    pub fn device_time(&self) -> u64 {
        self.lock().now()
    }
}

/// `nusb`'s error for a transfer that failed with an IOKit code it does not
/// map, as it prints it.
fn unknown(code: u32) -> U3vError {
    U3vError::usb(format!("unknown (error {code:#010x})"))
}

/// `kIOReturnNotResponding`, `kIOReturnNotReady`.
const NOT_RESPONDING: u32 = 0xe000_02ed;
const NOT_READY: u32 = 0xe000_02d8;

impl TransportState {
    /// How a transfer in direction IN (`true`) or OUT fails on an unplugged
    /// device.
    fn unplugged_error(&self, inbound: bool) -> U3vError {
        match (self.unplug_codes, inbound) {
            (false, _) => U3vError::Disconnected,
            (true, true) => unknown(NOT_RESPONDING),
            (true, false) => unknown(NOT_READY),
        }
    }

    fn now(&self) -> u64 {
        u64::try_from(self.clock.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }

    fn ready_to_stream(&self) -> bool {
        self.registers.stream_enabled() && self.acquiring
    }
}

impl UsbTransfer for FakeU3vTransport {
    fn bulk_write(
        &self,
        _endpoint: u8,
        data: &[u8],
        _timeout: Duration,
    ) -> Result<usize, U3vError> {
        let mut state = self.lock();
        if state.unplugged {
            return Err(state.unplugged_error(false));
        }
        state.pending_ack = execute(&mut state, data);
        Ok(data.len())
    }

    fn bulk_read(
        &self,
        endpoint: u8,
        buf: &mut [u8],
        timeout: Duration,
    ) -> Result<usize, U3vError> {
        let mut state = self.lock();
        if state.unplugged {
            return Err(state.unplugged_error(true));
        }

        let produced = match endpoint {
            // One read is one transfer queued.
            STREAM_EP => {
                state.max_host_queued = state.max_host_queued.max(1);
                stream_transfer(&mut state, buf.len(), 1)?
            }
            EVENT_EP => event_transfer(&mut state, buf.len())?,
            _ => {
                // Control endpoint: the acknowledge, if the command asked for one.
                let ack = state.pending_ack.take();
                if let Some(ack) = &ack
                    && buf.len() < ack.len()
                {
                    return Err(U3vError::usb(format!(
                        "overflow: a {}-byte acknowledge into a {}-byte buffer",
                        ack.len(),
                        buf.len()
                    )));
                }
                ack
            }
        };
        let Some(data) = produced else {
            drop(state);
            std::thread::sleep(timeout.min(IDLE_READ));
            return Err(U3vError::Timeout);
        };
        buf[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }

    fn bulk_in(self: &Arc<Self>, endpoint: u8) -> Result<Box<dyn BulkIn>, U3vError> {
        let state = self.lock();
        if state.unplugged {
            return Err(state.unplugged_error(true));
        }
        drop(state);
        Ok(Box::new(FakeBulkIn {
            transport: Arc::clone(self),
            endpoint,
            requested: VecDeque::new(),
            cancelled: 0,
        }))
    }

    fn max_packet_size(&self, _endpoint: u8) -> Option<usize> {
        Some(MAX_PACKET)
    }

    fn is_present(&self) -> Option<bool> {
        let state = self.lock();
        Some(!state.unplugged || state.listed_until.is_some_and(|t| Instant::now() < t))
    }
}

/// The fake's bulk IN queue: the device sees how many transfers are queued
/// on the stream endpoint as it sends.
struct FakeBulkIn {
    transport: Arc<FakeU3vTransport>,
    endpoint: u8,
    requested: VecDeque<usize>,
    /// How many of the oldest requests were cancelled.
    cancelled: usize,
}

impl FakeBulkIn {
    fn finish(&mut self, state: &mut TransportState) {
        self.requested.pop_front();
        if self.endpoint == STREAM_EP {
            state.host_queued = state.host_queued.saturating_sub(1);
        }
    }
}

impl BulkIn for FakeBulkIn {
    fn submit(&mut self, len: usize) {
        self.requested.push_back(len);
        if self.endpoint == STREAM_EP {
            let mut state = self.transport.lock();
            state.host_queued += 1;
            state.max_host_queued = state.max_host_queued.max(state.host_queued);
        }
    }

    fn pending(&self) -> usize {
        self.requested.len()
    }

    fn wait_next(&mut self, timeout: Duration) -> Option<Completion> {
        let len = *self.requested.front()?;
        let transport = Arc::clone(&self.transport);
        let mut state = transport.lock();
        if self.cancelled > 0 {
            self.cancelled -= 1;
            self.finish(&mut state);
            return Some(Completion::new(Vec::new(), Err(U3vError::Cancelled)));
        }
        if state.unplugged {
            self.finish(&mut state);
            let error = state.unplugged_error(true);
            return Some(Completion::new(Vec::new(), Err(error)));
        }
        let produced = match self.endpoint {
            STREAM_EP => {
                let queued = state.host_queued;
                stream_transfer(&mut state, len, queued)
            }
            EVENT_EP => event_transfer(&mut state, len),
            _ => Err(U3vError::Device("has no such bulk IN endpoint")),
        };
        match produced {
            Ok(Some(data)) => {
                self.finish(&mut state);
                Some(Completion::new(data, Ok(())))
            }
            Ok(None) => {
                drop(state);
                std::thread::sleep(timeout.min(IDLE_READ));
                None
            }
            Err(error) => {
                self.finish(&mut state);
                Some(Completion::new(Vec::new(), Err(error)))
            }
        }
    }

    fn cancel_all(&mut self) {
        self.cancelled = self.requested.len();
    }

    fn reset(&mut self) -> Result<(), U3vError> {
        let mut state = self.transport.lock();
        if state.unplugged {
            return Err(state.unplugged_error(true));
        }
        state.log.push(FakeUsbEvent::EndpointReset {
            endpoint: self.endpoint,
        });
        state.resets.push(self.endpoint);
        if self.endpoint == STREAM_EP {
            // A halt drops what the device had queued: the block in
            // progress. The next starts at its leader, and — as an
            // mvBlueFOX3-2004G does after the reset before StreamEnable —
            // block ids start again at 0.
            state.halted = false;
            state.block = None;
            state.next_block_id = 0;
        }
        if self.endpoint == EVENT_EP {
            state.events.clear();
        }
        Ok(())
    }

    fn is_present(&self) -> Option<bool> {
        self.transport.is_present()
    }
}

impl Drop for FakeBulkIn {
    fn drop(&mut self) {
        if self.endpoint == STREAM_EP {
            let mut state = self.transport.lock();
            state.host_queued = state.host_queued.saturating_sub(self.requested.len());
        }
    }
}

/// Execute one command packet; the acknowledge, or `None` when the command
/// did not ask for one or is not a GenCP command at all.
fn execute(state: &mut TransportState, packet: &[u8]) -> Option<Vec<u8>> {
    if packet.len() < HEADER_SIZE || packet[..4] != CONTROL_PREFIX {
        return None;
    }
    let le16 = |at: usize| u16::from_le_bytes([packet[at], packet[at + 1]]);
    let le64 = |at: usize| u64::from_le_bytes(packet[at..at + 8].try_into().unwrap());
    let (flags, command, length, request_id) = (le16(4), le16(6), le16(8), le16(10));
    if flags & REQUEST_ACK == 0 {
        return None;
    }
    let ack_id = command | 1;
    if packet.len() != HEADER_SIZE + length as usize {
        return Some(build_ack(
            StatusCode::InvalidHeader,
            ack_id,
            request_id,
            &[],
        ));
    }

    Some(match command {
        READMEM_CMD if length == 12 => {
            let (addr, reserved, count) = (le64(12), le16(20), le16(22) as usize);
            state.reads.push((addr, count));
            let status = if reserved != 0 {
                StatusCode::InvalidParameter
            } else if !state.registers.is_mapped(addr, count) {
                StatusCode::InvalidAddress
            } else if state.registers.is_write_only(addr, count) {
                StatusCode::AccessDenied
            } else {
                let data = state.registers.read(addr, count);
                return Some(build_ack(StatusCode::Success, ack_id, request_id, &data));
            };
            build_ack(status, ack_id, request_id, &[])
        }
        WRITEMEM_CMD if length >= 8 => {
            let (addr, data) = (le64(12), &packet[HEADER_SIZE + 8..]);
            let status = if request_id == 0 {
                // GenCP §3.1: request_id 0 is for reads only.
                StatusCode::InvalidParameter
            } else if !state.registers.is_mapped(addr, data.len()) {
                StatusCode::InvalidAddress
            } else if state.registers.is_read_only(addr, data.len()) {
                StatusCode::WriteProtect
            } else {
                state.registers.write(addr, data);
                state.writes.push((addr, data.to_vec()));
                state.log.push(FakeUsbEvent::Write {
                    address: addr,
                    data: data.to_vec(),
                });
                apply_side_effects(state, addr, data);
                // The Written Length Field capability: reserved, then the
                // bytes written (GenCP 1.2 Table 11).
                let mut scd = [0u8; 4];
                scd[2..].copy_from_slice(&(data.len() as u16).to_le_bytes());
                return Some(build_ack(StatusCode::Success, ack_id, request_id, &scd));
            };
            build_ack(status, ack_id, request_id, &[])
        }
        _ => build_ack(StatusCode::NotImplemented, ack_id, request_id, &[]),
    })
}

/// What a write starts or stops.
fn apply_side_effects(state: &mut TransportState, addr: u64, data: &[u8]) {
    let touches = |reg: u64| addr < reg + 4 && reg < addr + data.len() as u64;
    if touches(REG_ACQ_START) {
        state.acquiring = true;
    }
    if touches(REG_ACQ_STOP) {
        state.acquiring = false;
    }
    // A stream disabled mid-block abandons it.
    if touches(SIRM_CONTROL) && !state.registers.stream_enabled() {
        state.block = None;
    }
    // GenCP 1.3.1 §5.4.17: bit 0 latches the device time.
    if touches(ABRM_TIMESTAMP_LATCH) && data.first().is_some_and(|b| b & 1 != 0) {
        let now = state.now();
        state.registers.latch_timestamp(now);
    }
}

/// Build an acknowledge.
fn build_ack(status: StatusCode, command: u16, request_id: u16, scd: &[u8]) -> Vec<u8> {
    let mut buf = BytesMut::with_capacity(HEADER_SIZE + scd.len());
    buf.extend_from_slice(&CONTROL_PREFIX);
    buf.put_u16_le(status.to_raw());
    buf.put_u16_le(command);
    buf.put_u16_le(scd.len() as u16);
    buf.put_u16_le(request_id);
    buf.extend_from_slice(scd);
    buf.to_vec()
}

/// Bytes per pixel for the test pattern.
fn bytes_per_pixel(pixel_format: u32) -> usize {
    match pixel_format {
        0x0218_0014 => 3, // RGB8Packed
        _ => 1,           // Mono8 and others default to 1
    }
}

/// One host transfer of `len` bytes on the stream endpoint, with `queued`
/// transfers queued in all: `None` while the device has nothing to send.
///
/// Packets of up to [`MAX_PACKET`] bytes fill the transfer until it is full
/// or a short packet ends it. Reads width, height, pixel format and chunk
/// mode at each block's start, so a change takes effect on the next block.
fn stream_transfer(
    state: &mut TransportState,
    len: usize,
    queued: usize,
) -> Result<Option<Vec<u8>>, U3vError> {
    if state.halted {
        return Err(U3vError::Stalled {
            endpoint: STREAM_EP,
        });
    }
    if !state.ready_to_stream() {
        return Ok(None);
    }
    let mut out = Vec::with_capacity(len.min(1 << 20));
    loop {
        if state.block.is_none() {
            // Blocks start at the frame rate, as a camera's exposures do.
            let period = Duration::from_secs_f64(1.0 / state.registers.frame_rate().max(1.0));
            if state
                .last_block_start
                .is_some_and(|last| last.elapsed() < period)
            {
                if out.is_empty() {
                    return Ok(None);
                }
                break;
            }
            state.last_block_start = Some(Instant::now());
            start_block(state)?;
        }
        let required = state.required_queued;
        let block = state.block.as_mut().expect("started");
        let Some(transfer) = block.transfers.front_mut() else {
            state.block = None;
            continue;
        };
        if transfer.kind == Part::Payload && queued < required && !block.overrun {
            // Nowhere for the data to go: the device drops the payload.
            block.overrun = true;
            block.transfers.retain(|t| t.kind != Part::Payload);
            continue;
        }
        if transfer.kind == Part::Trailer && transfer.data.is_empty() {
            let status = if block.overrun { DATA_OVERRUN } else { 0 };
            transfer.data = build_trailer(block.id, status, block.sent as u64, block.height);
        }
        let packet_len = (transfer.data.len() - transfer.offset).min(MAX_PACKET);
        if out.len() + packet_len > len {
            if out.is_empty() {
                return Err(U3vError::usb(format!(
                    "overflow: a {packet_len}-byte packet into a {len}-byte transfer"
                )));
            }
            break;
        }
        if transfer.kind == Part::Payload
            && let Some(left) = state.stall_after.as_mut()
        {
            if *left == 0 {
                state.stall_after = None;
                state.halted = true;
                state.block = None;
                return Err(U3vError::Stalled {
                    endpoint: STREAM_EP,
                });
            }
            *left -= 1;
        }
        let block = state.block.as_mut().expect("started");
        let transfer = block.transfers.front_mut().expect("present");
        out.extend_from_slice(&transfer.data[transfer.offset..transfer.offset + packet_len]);
        transfer.offset += packet_len;
        if transfer.kind == Part::Payload {
            block.sent += packet_len;
        }
        let kind = transfer.kind;
        if transfer.offset == transfer.data.len() {
            block.transfers.pop_front();
            if kind == Part::Trailer {
                let (id, timestamp) = (block.id, block.timestamp);
                state.block = None;
                queue_exposure_end(state, id, timestamp);
            }
        }
        // A short packet ends the transfer, as does a full one.
        if packet_len < MAX_PACKET || out.len() == len {
            break;
        }
    }
    Ok(Some(out))
}

/// Start the next block: its leader, its payload in the transfer sizes the
/// host wrote to the SIRM, and a trailer.
fn start_block(state: &mut TransportState) -> Result<(), U3vError> {
    let layout = state.registers.host_layout();
    if layout.max_leader_size < LEADER_SIZE || layout.max_trailer_size < TRAILER_SIZE {
        return Err(U3vError::usb(format!(
            "the SIRM's leader/trailer sizes {}/{} are below the {LEADER_SIZE}/{TRAILER_SIZE} the device requires",
            layout.max_leader_size, layout.max_trailer_size
        )));
    }
    let (width, height) = (state.registers.width(), state.registers.height());
    let pixel_format = state.registers.pixel_format_code();
    let id = state.next_block_id;
    state.next_block_id += 1;
    let timestamp = state.now();

    let mut payload = generate_test_pattern(
        width as usize,
        height as usize,
        bytes_per_pixel(pixel_format),
        id as u16,
    );
    // A packed format's frame is not a whole number of bytes a pixel; its
    // image is the declared size whatever the pattern.
    let image_len = crate::registers::payload_bytes(width, height, pixel_format) as usize;
    payload.resize(image_len, 0x80);
    let chunked = state.registers.chunk_mode();
    if chunked {
        payload = chunk_payload(payload, &state.registers.enabled_chunks(), timestamp, id);
    }
    let payload_type: u16 = if chunked { 0x4001 } else { 0x0001 };

    let mut transfers = VecDeque::new();
    transfers.push_back(DeviceTransfer {
        kind: Part::Leader,
        data: build_leader(id, payload_type, timestamp, width, height, pixel_format),
        offset: 0,
    });
    let sizes = std::iter::repeat_n(
        layout.payload_transfer_size,
        layout.payload_transfer_count as usize,
    )
    .chain([layout.final_transfer1_size, layout.final_transfer2_size])
    .filter(|&size| size > 0);
    let mut at = 0;
    for size in sizes {
        if at >= payload.len() {
            break;
        }
        let end = (at + size as usize).min(payload.len());
        transfers.push_back(DeviceTransfer {
            kind: Part::Payload,
            data: payload[at..end].to_vec(),
            offset: 0,
        });
        at = end;
    }
    transfers.push_back(DeviceTransfer {
        kind: Part::Trailer,
        data: Vec::new(),
        offset: 0,
    });
    state.block = Some(Block {
        id,
        timestamp,
        transfers,
        sent: 0,
        overrun: false,
        height,
    });
    Ok(())
}

/// The image as the first chunk, then each enabled chunk, every tag
/// `[id u32][length u32]` little-endian after its data.
fn chunk_payload(image: Vec<u8>, enabled: &[u32], timestamp: u64, block_id: u64) -> Vec<u8> {
    let mut payload = image;
    let image_len = payload.len() as u32;
    payload.extend_from_slice(&CHUNK_ID_IMAGE.to_le_bytes());
    payload.extend_from_slice(&image_len.to_le_bytes());
    for &id in enabled {
        let value = match id {
            CHUNK_ID_TIMESTAMP => timestamp,
            CHUNK_ID_FRAME_ID => block_id,
            _ => 0,
        };
        payload.extend_from_slice(&value.to_le_bytes());
        payload.extend_from_slice(&id.to_le_bytes());
        payload.extend_from_slice(&8u32.to_le_bytes());
    }
    payload
}

/// Queue an ExposureEnd event for the block just sent, if the host asked
/// for it.
fn queue_exposure_end(state: &mut TransportState, block_id: u64, timestamp: u64) {
    if !(state.registers.events_enabled() && state.registers.exposure_end_notification()) {
        return;
    }
    state.event_request_id = state.event_request_id.wrapping_add(1).max(1);
    // GenCP 1.3.1 Table 17, MultiEvent off: event_size 0, then the event;
    // its data as an mvBlueFOX3-2004G sends it: the stream channel index,
    // then the frame ID.
    let mut scd = Vec::with_capacity(24);
    scd.extend_from_slice(&0u16.to_le_bytes());
    scd.extend_from_slice(&EVENT_ID_EXPOSURE_END.to_le_bytes());
    scd.extend_from_slice(&timestamp.to_le_bytes());
    scd.extend_from_slice(&0u32.to_le_bytes());
    scd.extend_from_slice(&block_id.to_le_bytes());
    let mut packet = BytesMut::with_capacity(HEADER_SIZE + scd.len());
    packet.extend_from_slice(&EVENT_PREFIX);
    packet.put_u16_le(0); // flags: no acknowledge asked for
    packet.put_u16_le(EVENT_CMD);
    packet.put_u16_le(scd.len() as u16);
    packet.put_u16_le(state.event_request_id);
    packet.extend_from_slice(&scd);
    state.events.push_back(packet.to_vec());
}

/// One host transfer of `len` bytes on the event endpoint.
fn event_transfer(state: &mut TransportState, len: usize) -> Result<Option<Vec<u8>>, U3vError> {
    if !state.registers.events_enabled() {
        return Ok(None);
    }
    let Some(packet) = state.events.pop_front() else {
        return Ok(None);
    };
    if packet.len() > len {
        return Err(U3vError::usb(format!(
            "overflow: a {}-byte event command into a {len}-byte transfer",
            packet.len()
        )));
    }
    Ok(Some(packet))
}

/// Mono8: concentric rings radiating from center, shifting with each frame.
/// RGB8: colorful plasma-like pattern with animated hue rotation.
fn generate_test_pattern(width: usize, height: usize, bpp: usize, frame: u16) -> Vec<u8> {
    let size = width * height * bpp;
    let mut data = vec![0u8; size];
    let cx = width as f32 / 2.0;
    let cy = height as f32 / 2.0;
    let phase = frame as f32 * 0.15;

    for y in 0..height {
        for x in 0..width {
            let offset = (y * width + x) * bpp;
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();

            if bpp == 1 {
                // Concentric rings with radial gradient
                let ring = ((dist * 0.1 - phase) * 2.0).sin();
                let radial = 1.0 - (dist / (cx.max(cy) * 1.2)).min(1.0);
                let val = ((ring * 0.5 + 0.5) * radial * 255.0) as u8;
                data[offset] = val;
            } else if bpp >= 3 {
                // Plasma pattern with hue rotation
                let angle = dy.atan2(dx);
                let v1 = ((dist * 0.05 - phase).sin() + 1.0) * 0.5;
                let v2 = ((angle * 3.0 + phase * 2.0).sin() + 1.0) * 0.5;
                let v3 = (((x as f32 * 0.02 + y as f32 * 0.03 + phase).sin()) + 1.0) * 0.5;

                let hue = (v1 + v2) * 0.5 + phase * 0.1;
                let sat = 0.7 + v3 * 0.3;
                let val = 0.5 + v1 * 0.5;
                let (r, g, b) = hsv_to_rgb(hue % 1.0, sat, val);
                data[offset] = r;
                data[offset + 1] = g;
                data[offset + 2] = b;
            }
        }
    }
    data
}

/// Convert HSV (all 0.0..1.0) to RGB (0..255).
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let h = h * 6.0;
    let i = h.floor() as u32;
    let f = h - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    let (r, g, b) = match i % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// A 52-byte image leader, as Wireshark's `dissect_u3v_stream_leader`
/// reads one.
fn build_leader(
    block_id: u64,
    payload_type: u16,
    timestamp: u64,
    width: u32,
    height: u32,
    pixel_format: u32,
) -> Vec<u8> {
    let mut buf = vec![0u8; LEADER_SIZE as usize];
    buf[0..4].copy_from_slice(&LEADER_PREFIX);
    buf[6..8].copy_from_slice(&(LEADER_SIZE as u16).to_le_bytes());
    buf[8..16].copy_from_slice(&block_id.to_le_bytes());
    buf[18..20].copy_from_slice(&payload_type.to_le_bytes());
    buf[20..28].copy_from_slice(&timestamp.to_le_bytes());
    buf[28..32].copy_from_slice(&pixel_format.to_le_bytes());
    buf[32..36].copy_from_slice(&width.to_le_bytes());
    buf[36..40].copy_from_slice(&height.to_le_bytes());
    buf
}

/// A 32-byte image trailer, as `dissect_u3v_stream_trailer` reads one.
fn build_trailer(block_id: u64, status: u16, valid_payload_size: u64, height: u32) -> Vec<u8> {
    let mut buf = vec![0u8; TRAILER_SIZE as usize];
    buf[0..4].copy_from_slice(&TRAILER_PREFIX);
    buf[6..8].copy_from_slice(&(TRAILER_SIZE as u16).to_le_bytes());
    buf[8..16].copy_from_slice(&block_id.to_le_bytes());
    buf[16..18].copy_from_slice(&status.to_le_bytes());
    buf[20..28].copy_from_slice(&valid_payload_size.to_le_bytes());
    buf[28..32].copy_from_slice(&height.to_le_bytes());
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registers::{REG_ACQ_START, SIRM_BASE, SIRM_REQUIRED_LEADER_SIZE};
    use std::sync::Arc;
    use viva_u3v::U3vDevice;
    use viva_u3v::U3vStatus;

    fn open(transport: &Arc<FakeU3vTransport>) -> U3vDevice<FakeU3vTransport> {
        crate::open(Arc::clone(transport)).unwrap()
    }

    #[test]
    fn fake_transport_open_device() {
        let transport = Arc::new(FakeU3vTransport::new(640, 480, 0x0108_0001));
        let device = open(&transport);
        assert_eq!(device.abrm().manufacturer_name, "FakeCorp");
        assert_eq!(device.abrm().model_name, "FakeU3V");
        assert_eq!(device.abrm().family_name, "Test");
        assert_eq!(device.sbrm().max_cmd_transfer, 4096);
        assert_eq!(device.sbrm().sirm_address, SIRM_BASE);
    }

    #[test]
    fn fake_transport_fetch_xml() {
        let transport = Arc::new(FakeU3vTransport::new(320, 240, 0x0108_0001));
        let xml = open(&transport).fetch_xml().unwrap();
        assert!(xml.contains("RegisterDescription"));
        assert!(xml.contains("Width"));
    }

    /// The fake refuses what a device refuses, and records the attempt.
    #[test]
    fn a_write_only_register_read_is_refused_and_recorded() {
        let transport = Arc::new(FakeU3vTransport::new(64, 64, 0x0108_0001));
        let mut device = open(&transport);
        transport.clear_reads();

        let err = device
            .read_mem(REG_ACQ_START, 4)
            .expect_err("a WO register must refuse a read");
        assert!(
            matches!(
                err,
                U3vError::Status(U3vStatus::Common(StatusCode::AccessDenied))
            ),
            "{err:?}"
        );
        assert!(transport.was_read(REG_ACQ_START));

        device
            .write_mem(REG_ACQ_START, &1u32.to_le_bytes())
            .expect("a WO register still accepts a write");
    }

    #[test]
    fn a_required_size_refuses_a_write() {
        let transport = Arc::new(FakeU3vTransport::new(64, 64, 0x0108_0001));
        let err = open(&transport)
            .write_mem(SIRM_REQUIRED_LEADER_SIZE, &64u32.to_le_bytes())
            .unwrap_err();
        assert!(
            matches!(
                err,
                U3vError::Status(U3vStatus::Common(StatusCode::WriteProtect))
            ),
            "{err:?}"
        );
    }

    #[test]
    fn an_unmapped_address_is_refused() {
        let transport = Arc::new(FakeU3vTransport::new(64, 64, 0x0108_0001));
        let err = open(&transport).read_mem(0x0228, 8).unwrap_err();
        assert!(
            matches!(
                err,
                U3vError::Status(U3vStatus::Common(StatusCode::InvalidAddress))
            ),
            "{err:?}"
        );
    }

    /// Nothing streams until the host has enabled the stream and started
    /// acquisition; then blocks arrive in the configured transfers.
    #[test]
    fn a_stream_flows_only_when_enabled_and_acquiring() {
        let transport = Arc::new(FakeU3vTransport::new(64, 48, 0x0108_0001));
        let mut device = open(&transport);
        let mut stream = device.open_stream(0).unwrap();
        assert!(transport.stream_enabled());
        assert!(matches!(
            stream.next_frame_within(Duration::from_millis(1)),
            Ok(None)
        ));

        device
            .write_mem(REG_ACQ_START, &1u32.to_le_bytes())
            .unwrap();
        let frame = stream.next_frame().unwrap();
        assert_eq!((frame.leader.width, frame.leader.height), (64, 48));
        assert_eq!(frame.payload.len(), 64 * 48);
        assert_eq!(
            frame.trailer.expect("a trailer").block_id,
            frame.leader.block_id
        );

        device.stop_stream().unwrap();
        assert!(!transport.stream_enabled());
    }
}
