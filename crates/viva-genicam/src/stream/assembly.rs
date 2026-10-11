//! Socket-free GVSP block assembly (ADR-0025).
//!
//! [`Assembler`] turns datagrams into blocks. It owns no socket and no clock:
//! the receive thread hands it each datagram with the instant it arrived, and
//! calls [`Assembler::poll`] to let timeouts fire. That is what lets every rule
//! below be tested with scripted packets and invented instants.
//!
//! - **Frames in flight.** Up to [`MAX_IN_FLIGHT`] blocks assemble at once,
//!   keyed by block id, so interleaved and reordered blocks both complete. A
//!   new block beyond that closes the oldest.
//! - **In order.** Blocks leave in the order they started, so a block that
//!   completes while an older one still waits is held until that one closes.
//! - **Completion** needs the leader, the trailer and every data packet the
//!   trailer accounts for, in any order. A trailer that arrives early only
//!   records how many packets there are.
//! - **Inactivity timeout.** A block closes incomplete when nothing has arrived
//!   for it for the block timeout (default [`DEFAULT_BLOCK_TIMEOUT`]), counted
//!   from its **last** packet — a slow transfer with small gaps completes.
//! - **Stride.** A data packet carries `packet_size − 20 − 8 − header` bytes,
//!   with the header length (8, or 20 with extended ids) read from each packet.
//! - **Buffer.** Allocated once, at `PayloadSize` when the stream knows it,
//!   else from the leader's geometry (capped at [`ESTIMATE_CAP`]); each packet
//!   is written in place at `(packet id − 1) × stride`. `PayloadSize` sizes the
//!   buffer but does not bound it: one read before a ROI change grows rather
//!   than failing every frame.
//! - **Block ids** are compared wrap-aware. A forward jump counts the skipped
//!   ids as missed frames; a block a little behind the newest is late; a long
//!   way behind, or back to 1, is a restart.
//! - **Resend.** With resend on, each block tracks its gaps
//!   ([`ResendState`]); [`Assembler::resend_due`] says which ranges to ask
//!   for, and a camera's "no longer available" answer closes a range.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::resend::{Answer, ResendConfig, ResendRequest, ResendState};
use super::stats::{IgnoredPacket, StreamStatsAccumulator};
use bytes::Bytes;
use tracing::{debug, warn};
use viva_gige::gvsp::{
    GVSP_EXTENDED_HEADER_SIZE, GVSP_HEADER_SIZE, GvspContent, GvspHeader, GvspLeader, GvspTrailer,
    PacketBitmap, is_chunk_data_payload, is_image_payload, parse_header, parse_packet_ref, status,
};

use crate::frame::FrameStatus;

/// IPv4 header counted by `GevSCPSPacketSize`.
const IPV4_HEADER_LEN: usize = 20;
/// UDP header counted by `GevSCPSPacketSize`.
const UDP_HEADER_LEN: usize = 8;

/// Blocks assembled at once. A fourth closes the oldest.
pub(crate) const MAX_IN_FLIGHT: usize = 3;

/// Inactivity after which an unfinished block closes. aravis's
/// `frame-retention` default is 100 ms too, also measured from the last
/// packet (`arvgvstream.c`, `_check_frame_completion`) — corroboration only.
pub(crate) const DEFAULT_BLOCK_TIMEOUT: Duration = Duration::from_millis(100);

/// How far behind the newest block id a block may be and still count as late
/// rather than as the camera restarting its count. aravis uses the same 100
/// (`ARV_GV_STREAM_DISCARD_LATE_FRAME_THRESHOLD`).
const LATE_WINDOW: u64 = 100;

/// Largest buffer sized from a leader's geometry when `PayloadSize` is not
/// known. A leader is remote input; it does not get to allocate gigabytes.
pub(crate) const ESTIMATE_CAP: usize = 256 << 20;

/// Bit 15 of a GVSP status: an error.
const STATUS_ERROR: u16 = 0x8000;

/// Image bytes one data packet carries at `packet_size` with a `header_len`
/// GVSP header.
pub(crate) fn payload_stride(packet_size: u32, header_len: usize) -> usize {
    (packet_size as usize).saturating_sub(IPV4_HEADER_LEN + UDP_HEADER_LEN + header_len)
}

/// Bytes an image occupies, from the leader: `size_y` lines of
/// `ceil(size_x × bits / 8) + padding_x` bytes, plus `padding_y`. Bits 23–16
/// of the PFNC code are the pixel depth, which is what makes packed formats
/// (12 bits a pixel) come out right.
fn geometry_size(image: &viva_gige::gvsp::ImageLeader) -> usize {
    let bits = ((image.pixel_format >> 16) & 0xFF) as u64;
    let bits = if bits == 0 { 8 } else { bits };
    let line = (image.width as u64 * bits).div_ceil(8) + u64::from(image.padding_x);
    let total = line
        .saturating_mul(u64::from(image.height))
        .saturating_add(u64::from(image.padding_y));
    usize::try_from(total).unwrap_or(usize::MAX)
}

/// Where frame buffers come from.
///
/// The default allocates. A GenTL producer (milestone T) hands in the buffers
/// its consumer announced instead, which is why the assembler is handed a
/// buffer rather than allocating one itself.
pub(crate) trait BufferSource: Send {
    /// A zeroed buffer of exactly `len` bytes.
    fn acquire(&mut self, len: usize) -> Vec<u8>;
}

/// The default [`BufferSource`]: a fresh zeroed allocation per frame.
#[derive(Debug, Default)]
pub(crate) struct Allocate;

impl BufferSource for Allocate {
    fn acquire(&mut self, len: usize) -> Vec<u8> {
        vec![0; len]
    }
}

/// What the assembler needs to know about the stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AssemblyConfig {
    /// `GevSCPSPacketSize` in force.
    pub packet_size: u32,
    /// `PayloadSize`, when known: the exact buffer size.
    pub payload_size: Option<usize>,
    /// Inactivity after which an unfinished block closes.
    pub block_timeout: Duration,
    /// Whether and how to ask for lost packets.
    pub resend: ResendConfig,
}

impl AssemblyConfig {
    /// A stream at `packet_size` with nothing else known.
    pub(crate) fn new(packet_size: u32) -> Self {
        Self {
            packet_size,
            payload_size: None,
            block_timeout: DEFAULT_BLOCK_TIMEOUT,
            resend: ResendConfig::default(),
        }
    }
}

/// The leader fields a frame keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LeaderInfo {
    pub timestamp: u64,
    pub pixel_format: u32,
    pub width: u32,
    pub height: u32,
    /// Bytes after every line of the image.
    pub padding_x: u16,
}

/// A block the assembler is finished with.
#[derive(Debug)]
pub(crate) struct AssembledBlock {
    pub block_id: u64,
    pub status: FrameStatus,
    /// `None` when the leader was lost.
    pub leader: Option<LeaderInfo>,
    /// From the leader, else the trailer, else 0.
    pub payload_type: u16,
    /// Bytes up to the end of the furthest packet received; lost packets'
    /// bytes are zero.
    pub payload: Bytes,
    /// Size Y from the trailer, 0 without one.
    pub trailer_size_y: u32,
}

#[derive(Debug)]
struct TrailerInfo {
    /// Packet id of the last data packet: the trailer's id minus one.
    last_payload_id: u32,
    size_y: u32,
    payload_type: u16,
}

/// How a block in flight was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolution {
    Frame(FrameStatus),
    /// A payload type the engine does not assemble.
    Unsupported,
}

#[derive(Debug)]
struct BlockInFlight {
    block_id: u64,
    /// Whether the block's packets carry extended ids.
    extended_ids: bool,
    /// Gaps to ask for, when resend is on and the block is assembled.
    resend: Option<ResendState>,
    last_packet: Instant,
    leader: Option<LeaderInfo>,
    leader_payload_type: Option<u16>,
    /// Set when the leader names a payload type that is not assembled.
    unsupported: bool,
    trailer: Option<TrailerInfo>,
    buffer: Vec<u8>,
    /// End of the furthest byte written.
    extent: usize,
    /// Data packets received, indexed by packet id − 1.
    bitmap: PacketBitmap,
    highest_payload_id: u32,
    any_payload: bool,
    /// A data packet shorter than the stride whose place as the last packet
    /// is not yet disproved: `(packet id, length, stride)`.
    short_packet: Option<(u32, usize, usize)>,
    resolved: Option<Resolution>,
}

/// Where a packet's block stands relative to the blocks already seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Order {
    /// The first block of the stream, or id 0 (outside the sequence).
    Unordered,
    /// Newer than any so far; `missed` ids were skipped.
    Next { missed: u64 },
    /// Older than the newest, within [`LATE_WINDOW`]: closed or never seen.
    Late,
    /// The camera started counting again.
    Restart,
}

/// Signed distance from `last` to `next` in the 16-bit id cycle 1..=65535,
/// which skips 0. Positive is newer; the cycle is split at its midpoint.
fn distance16(last: u64, next: u64) -> i64 {
    const CYCLE: i64 = 65535;
    let pos = |id: u64| (id as i64 - 1).rem_euclid(CYCLE);
    let d = (pos(next) - pos(last)).rem_euclid(CYCLE);
    if d > CYCLE / 2 { d - CYCLE } else { d }
}

/// Classify `next` against the newest block id so far.
fn classify(last: Option<u64>, next: u64, extended_ids: bool, leader: bool) -> Order {
    let Some(last) = last else {
        return Order::Unordered;
    };
    if next == 0 && !extended_ids {
        return Order::Unordered;
    }
    let d = if extended_ids {
        next.wrapping_sub(last) as i64
    } else {
        distance16(last, next)
    };
    // Block ids start at 1. A *leader* of block 1 from anything but its
    // predecessor is a camera that restarted acquisition and its count;
    // without this a restart after fewer than LATE_WINDOW blocks would discard
    // the new blocks as late, and in 16-bit mode a restart past id 32 768
    // would read as a huge gap. Only a leader starts a block: a stray data
    // packet or duplicate of an old block 1 is late, not a new acquisition.
    if next == 1 && leader && d != 1 && d != 0 {
        return Order::Restart;
    }
    if d > 0 {
        Order::Next {
            missed: d as u64 - 1,
        }
    } else if d.unsigned_abs() <= LATE_WINDOW {
        Order::Late
    } else {
        Order::Restart
    }
}

/// A packet's content, once the formats the engine does not assemble are out.
enum Content<'a> {
    Leader(GvspLeader),
    Payload(&'a [u8]),
    Trailer(GvspTrailer),
}

/// Warnings worth saying once per stream rather than once per packet.
#[derive(Debug, Default)]
struct Warned {
    size_mismatch: bool,
    out_of_range: bool,
    unsupported: bool,
}

/// How a GVSP status that answers a resend request is acted on, or `None`
/// for any other status. The names are Wireshark's `GEV_STATUS_*`. Gone and
/// gone-with-previous close the range; "not yet available" and "temporarily
/// unavailable" leave it to its retries.
fn resend_answer(status: u16) -> Option<Option<Answer>> {
    match status {
        status::PACKET_UNAVAILABLE | status::PACKET_REMOVED_FROM_MEMORY => Some(Some(Answer::Gone)),
        status::PACKET_AND_PREVIOUS_REMOVED_FROM_MEMORY => Some(Some(Answer::GoneWithPrevious)),
        status::PACKET_NOT_YET_AVAILABLE | status::PACKET_TEMPORARILY_UNAVAILABLE => Some(None),
        _ => None,
    }
}

/// Assembles GVSP blocks from datagrams. See the module documentation.
pub(crate) struct Assembler {
    config: AssemblyConfig,
    stats: StreamStatsAccumulator,
    buffers: Box<dyn BufferSource>,
    in_flight: VecDeque<BlockInFlight>,
    last_block_id: Option<u64>,
    spare_bitmaps: Vec<PacketBitmap>,
    largest_estimate: usize,
    warned: Warned,
}

impl Assembler {
    pub(crate) fn new(config: AssemblyConfig, stats: StreamStatsAccumulator) -> Self {
        Self::with_buffers(config, stats, Box::new(Allocate))
    }

    pub(crate) fn with_buffers(
        config: AssemblyConfig,
        stats: StreamStatsAccumulator,
        buffers: Box<dyn BufferSource>,
    ) -> Self {
        Self {
            config,
            stats,
            buffers,
            in_flight: VecDeque::with_capacity(MAX_IN_FLIGHT + 1),
            last_block_id: None,
            spare_bitmaps: Vec::new(),
            largest_estimate: 0,
            warned: Warned::default(),
        }
    }

    pub(crate) fn config(&self) -> AssemblyConfig {
        self.config
    }

    /// Change the configuration. Blocks already in flight keep their buffers.
    pub(crate) fn set_config(&mut self, config: AssemblyConfig) {
        self.config = config;
    }

    /// The largest frame the stream has announced: `PayloadSize`, or the
    /// biggest leader geometry seen. For sizing the socket's receive buffer.
    pub(crate) fn frame_size_hint(&self) -> Option<usize> {
        match self.config.payload_size {
            Some(size) => Some(size.max(self.largest_estimate)),
            None if self.largest_estimate > 0 => Some(self.largest_estimate),
            None => None,
        }
    }

    /// Blocks currently assembling or waiting to leave in order.
    #[cfg(test)]
    pub(crate) fn in_flight(&self) -> usize {
        self.in_flight.len()
    }

    /// Take one datagram that arrived at `now`. Blocks that finish are
    /// appended to `out` in the order they started.
    pub(crate) fn ingest(&mut self, datagram: &[u8], now: Instant, out: &mut Vec<AssembledBlock>) {
        // A camera that cannot resend a packet answers with a bare header, so
        // the status is read before anything else is parsed.
        if let Ok(header) = parse_header(datagram)
            && let Some(answer) = resend_answer(header.status)
        {
            self.touch(header.block_id, now);
            self.stats.record_ignored_packet(IgnoredPacket::ErrorStatus);
            self.on_resend_answer(&header, answer);
            return;
        }
        let packet = match parse_packet_ref(datagram) {
            Ok(packet) => packet,
            Err(err) => {
                debug!(error = %err, len = datagram.len(), "ignoring a datagram that is not GVSP");
                self.stats.record_ignored_packet(IgnoredPacket::Malformed);
                return;
            }
        };
        let header = packet.header;
        let content = match packet.content {
            GvspContent::Leader(leader) => Content::Leader(leader),
            GvspContent::Payload(data) => Content::Payload(data),
            GvspContent::Trailer(trailer) => Content::Trailer(trailer),
            other => {
                let format = match other {
                    GvspContent::Unsupported { packet_format } => packet_format,
                    _ => 0,
                };
                self.touch(header.block_id, now);
                self.ignore_unsupported(format_args!(
                    "GVSP packet format {format:#04x} (block {}) is not assembled",
                    header.block_id
                ));
                return;
            }
        };

        let is_trailer = matches!(content, Content::Trailer(_));
        if !is_trailer && header.status & STATUS_ERROR != 0 {
            self.touch(header.block_id, now);
            debug!(
                block_id = header.block_id,
                packet_id = header.packet_id,
                status = format_args!("{:#06x}", header.status),
                "ignoring a GVSP packet whose status reports an error"
            );
            self.stats.record_ignored_packet(IgnoredPacket::ErrorStatus);
            return;
        }

        let is_leader = matches!(content, Content::Leader(_));
        let Some(index) = self.admit(&header, is_leader, now, out) else {
            self.stats.record_ignored_packet(IgnoredPacket::Late);
            return;
        };
        let block = &mut self.in_flight[index];
        block.last_packet = now;
        match block.resolved {
            // Every packet of a complete block is in; this one is a repeat.
            Some(Resolution::Frame(FrameStatus::Complete)) => {
                self.stats.record_duplicate_packet();
                return;
            }
            // Closed and waiting for an older block to leave first.
            Some(_) => {
                self.stats.record_ignored_packet(IgnoredPacket::Late);
                return;
            }
            None => {}
        }

        let arrived = match content {
            Content::Leader(leader) => {
                self.on_leader(index, leader);
                true
            }
            Content::Payload(data) => self.on_payload(index, &header, data),
            Content::Trailer(trailer) => {
                self.on_trailer(index, &header, trailer);
                true
            }
        };
        if arrived {
            self.track_arrival(index, header.packet_id, is_trailer, now);
        }
        self.try_complete(index);
        self.emit(out);
    }

    /// Tell the block's resend state that packet `id` arrived.
    fn track_arrival(&mut self, index: usize, id: u32, is_trailer: bool, now: Instant) {
        let delay = self.config.resend.delay;
        let Some(resend) = self.in_flight[index].resend.as_mut() else {
            return;
        };
        if resend.arrived(id, now, delay) {
            self.stats.record_resend_recovered();
        }
        if is_trailer {
            resend.trailer(id);
        }
    }

    /// A camera's answer to a resend request it could not serve.
    fn on_resend_answer(&mut self, header: &GvspHeader, answer: Option<Answer>) {
        let block = self
            .in_flight
            .iter_mut()
            .find(|b| b.block_id == header.block_id && b.resolved.is_none());
        let (Some(block), Some(answer)) = (block, answer) else {
            debug!(
                block_id = header.block_id,
                packet_id = header.packet_id,
                status = format_args!("{:#06x}", header.status),
                "resend answer for a block not in flight, or one to retry"
            );
            return;
        };
        let Some(resend) = block.resend.as_mut() else {
            return;
        };
        let gone = resend.unavailable(header.packet_id, answer);
        debug!(
            block_id = header.block_id,
            packet_id = header.packet_id,
            status = format_args!("{:#06x}", header.status),
            gone,
            "the camera cannot resend these packets"
        );
        if gone > 0 {
            self.stats.record_resend_unavailable(gone);
        }
    }

    /// Append the resend requests due at `now`, in block order.
    pub(crate) fn resend_due(&mut self, now: Instant, out: &mut Vec<ResendRequest>) {
        let config = self.config.resend;
        if !config.enabled {
            return;
        }
        let packet_size = self.config.packet_size;
        for block in self.in_flight.iter_mut() {
            if block.resolved.is_some() {
                continue;
            }
            let Some(resend) = block.resend.as_mut() else {
                continue;
            };
            let header_len = if block.extended_ids {
                GVSP_EXTENDED_HEADER_SIZE
            } else {
                GVSP_HEADER_SIZE
            };
            let stride = payload_stride(packet_size, header_len).max(1);
            // Leader, data packets and trailer: from the trailer once it has
            // come, else from the buffer, else from what has been seen.
            let packets = match &block.trailer {
                Some(t) => u64::from(t.last_payload_id) + 2,
                None if !block.buffer.is_empty() => block.buffer.len().div_ceil(stride) as u64 + 2,
                None => u64::from(resend.frontier()) + 1,
            };
            let done = resend.due(
                now,
                &config,
                packets,
                (block.block_id, block.extended_ids),
                out,
            );
            if done.newly_requested > 0 {
                self.stats.record_resend_requested(done.newly_requested);
            }
            if done.budget_spent {
                debug!(
                    block_id = block.block_id,
                    packets,
                    budget = config.budget,
                    "resend budget spent: no more requests for this block"
                );
            }
        }
    }

    /// Close every block that has been silent for the block timeout.
    pub(crate) fn poll(&mut self, now: Instant, out: &mut Vec<AssembledBlock>) {
        let timeout = self.config.block_timeout;
        let newest = self.in_flight.len().saturating_sub(1);
        for index in 0..self.in_flight.len() {
            let block = &self.in_flight[index];
            if block.resolved.is_some()
                || now.saturating_duration_since(block.last_packet) < timeout
            {
                continue;
            }
            // aravis does not time out the newest block while its leader is
            // the only packet received: some devices send the leader early, at
            // acquisition start (`_check_frame_completion`). A newer block, or
            // the in-flight limit, still closes it.
            if index == newest
                && block.leader.is_some()
                && !block.any_payload
                && block.trailer.is_none()
            {
                continue;
            }
            self.close(index);
        }
        self.emit(out);
    }

    fn touch(&mut self, block_id: u64, now: Instant) {
        if let Some(block) = self.in_flight.iter_mut().find(|b| b.block_id == block_id) {
            block.last_packet = now;
        }
    }

    fn ignore_unsupported(&mut self, what: std::fmt::Arguments<'_>) {
        if !self.warned.unsupported {
            self.warned.unsupported = true;
            warn!(
                "{what}; the receive engine assembles images (0x0001, 0x4001) and chunk \
                 data (0x0004, 0x4004). Such packets are counted in packets_ignored_unsupported"
            );
        }
        self.stats.record_ignored_packet(IgnoredPacket::Unsupported);
    }

    /// The index of the packet's block, starting it if it is new; `None` for
    /// a late packet.
    fn admit(
        &mut self,
        header: &GvspHeader,
        leader: bool,
        now: Instant,
        out: &mut Vec<AssembledBlock>,
    ) -> Option<usize> {
        if let Some(index) = self
            .in_flight
            .iter()
            .position(|b| b.block_id == header.block_id)
        {
            return Some(index);
        }
        match classify(
            self.last_block_id,
            header.block_id,
            header.extended_ids,
            leader,
        ) {
            Order::Late => return None,
            Order::Next { missed } => {
                if missed > 0 {
                    debug!(
                        missed,
                        before = header.block_id,
                        "block ids skipped: blocks the camera sent that never arrived"
                    );
                    self.stats.record_missed_frames(missed);
                }
                self.last_block_id = Some(header.block_id);
            }
            Order::Restart => {
                debug!(
                    previous = self.last_block_id,
                    block_id = header.block_id,
                    "block ids restarted"
                );
                self.last_block_id = Some(header.block_id);
            }
            Order::Unordered => {
                if header.block_id != 0 || header.extended_ids {
                    self.last_block_id = Some(header.block_id);
                }
            }
        }

        // The in-flight limit: the oldest block gives way, closed with
        // whatever it has.
        while self.in_flight.len() >= MAX_IN_FLIGHT {
            if self.in_flight[0].resolved.is_none() {
                self.close(0);
            }
            self.emit(out);
        }

        let stride = payload_stride(self.config.packet_size, header.header_len).max(1);
        let (buffer, packets) = match self.config.payload_size {
            Some(size) => (self.buffers.acquire(size), size.div_ceil(stride)),
            None => (Vec::new(), 0),
        };
        let mut bitmap = self.spare_bitmaps.pop().unwrap_or_default();
        bitmap.reset(packets);
        self.in_flight.push_back(BlockInFlight {
            block_id: header.block_id,
            extended_ids: header.extended_ids,
            resend: self.config.resend.enabled.then(ResendState::default),
            last_packet: now,
            leader: None,
            leader_payload_type: None,
            unsupported: false,
            trailer: None,
            buffer,
            extent: 0,
            bitmap,
            highest_payload_id: 0,
            any_payload: false,
            short_packet: None,
            resolved: None,
        });
        Some(self.in_flight.len() - 1)
    }

    fn on_leader(&mut self, index: usize, leader: GvspLeader) {
        let block = &mut self.in_flight[index];
        if block.leader_payload_type.is_some() {
            // A repeated leader for a block in flight changes nothing.
            self.stats.record_duplicate_packet();
            return;
        }
        block.leader_payload_type = Some(leader.payload_type);
        if is_chunk_data_payload(leader.payload_type) {
            // Chunk data (`0x0004`) has no geometry to size the buffer from:
            // it is `PayloadSize` when the stream knows it, and otherwise
            // grows with the packets, as any block's does past an estimate.
            block.leader = Some(LeaderInfo {
                timestamp: leader.timestamp,
                pixel_format: 0,
                width: 0,
                height: 0,
                padding_x: 0,
            });
            return;
        }
        let Some(image) = leader
            .image
            .filter(|_| is_image_payload(leader.payload_type))
        else {
            block.unsupported = true;
            block.resend = None;
            block.buffer = Vec::new();
            let payload_type = leader.payload_type;
            let block_id = block.block_id;
            self.ignore_unsupported(format_args!(
                "GVSP payload type {payload_type:#06x} (block {block_id}) is not assembled"
            ));
            return;
        };
        block.leader = Some(LeaderInfo {
            timestamp: leader.timestamp,
            pixel_format: image.pixel_format,
            width: image.width,
            height: image.height,
            padding_x: image.padding_x,
        });
        // `PayloadSize` normally covers the geometry; a stale one read before
        // a ROI or format change does not, and the leader wins.
        let estimate = geometry_size(&image).min(ESTIMATE_CAP);
        self.largest_estimate = self.largest_estimate.max(estimate);
        if block.buffer.len() < estimate {
            if block.buffer.is_empty() {
                block.buffer = self.buffers.acquire(estimate);
            } else {
                block.buffer.resize(estimate, 0);
            }
        }
    }

    /// Bytes a block's buffer may grow to: [`ESTIMATE_CAP`], or `PayloadSize`
    /// if the camera declares more.
    fn buffer_limit(&self) -> usize {
        ESTIMATE_CAP.max(self.config.payload_size.unwrap_or(0))
    }

    /// Place a data packet. Returns whether it counts as having arrived for
    /// resend: a packet beyond the frame does not, a refused one does — the
    /// camera would only send the same bytes again.
    fn on_payload(&mut self, index: usize, header: &GvspHeader, data: &[u8]) -> bool {
        let packet_size = self.config.packet_size;
        let limit = self.buffer_limit() as u64;
        let block = &mut self.in_flight[index];
        if block.unsupported {
            self.stats.record_ignored_packet(IgnoredPacket::Unsupported);
            return false;
        }
        let packet_id = header.packet_id;
        let stride = payload_stride(packet_size, header.header_len);
        if packet_id == 0 || stride == 0 {
            self.stats.record_ignored_packet(IgnoredPacket::Malformed);
            return false;
        }
        if block
            .trailer
            .as_ref()
            .is_some_and(|t| packet_id > t.last_payload_id)
        {
            self.out_of_range(index, packet_id, data.len());
            return false;
        }
        let slot = (packet_id - 1) as usize;
        if block.bitmap.contains(slot) {
            self.stats.record_duplicate_packet();
            return true;
        }
        if data.len() > stride {
            // Written at our stride it would overlap the next packet: the two
            // ends disagree about the packet size (SR-02). Refusing it leaves
            // the frame incomplete, which is visible, rather than complete and
            // scrambled, which is not.
            let block_id = block.block_id;
            self.size_mismatch(block_id, packet_id, data.len(), stride);
            return true;
        }
        let offset = slot as u64 * stride as u64;
        let end = offset + data.len() as u64;
        if end > limit {
            self.out_of_range(index, packet_id, data.len());
            return false;
        }
        let (offset, end) = (offset as usize, end as usize);
        if end > block.buffer.len() {
            block.buffer.resize(end, 0);
        }
        block.buffer[offset..end].copy_from_slice(data);
        block.extent = block.extent.max(end);
        block.bitmap.grow(slot + 1);
        block.bitmap.set(slot);
        block.any_payload = true;
        block.highest_payload_id = block.highest_payload_id.max(packet_id);

        // A short packet is legitimate only as the block's last. Of two short
        // packets the lower cannot be last; the higher stays pending until a
        // packet beyond it, or the trailer, shows it is not last either.
        let known_last = block.trailer.as_ref().map(|t| t.last_payload_id);
        let mut mismatches: Vec<(u32, usize, usize)> = Vec::new();
        if data.len() < stride {
            let this = (packet_id, data.len(), stride);
            match block.short_packet {
                Some(pending) if pending.0 < packet_id => {
                    mismatches.push(pending);
                    block.short_packet = Some(this);
                }
                Some(_) => mismatches.push(this),
                None => block.short_packet = Some(this),
            }
        }
        if let Some(pending) = block.short_packet
            && (pending.0 < block.highest_payload_id
                || known_last.is_some_and(|last| pending.0 < last))
        {
            block.short_packet = None;
            mismatches.push(pending);
        }
        for (id, len, stride) in mismatches {
            self.refuse_short(index, id, len, stride);
        }
        true
    }

    /// A data packet shorter than the stride turned out not to be the last:
    /// the camera's stride is not ours, so its bytes sit in the wrong place.
    /// It is taken back out — bit cleared, bytes zeroed — so the frame closes
    /// incomplete instead of completing with shifted data (Codex review of
    /// #194). The camera would send the same bytes again, so it is not asked
    /// for.
    fn refuse_short(&mut self, index: usize, packet_id: u32, len: usize, stride: usize) {
        let block = &mut self.in_flight[index];
        let slot = (packet_id - 1) as usize;
        if block.bitmap.clear(slot) {
            let offset = slot * stride;
            let end = (offset + len).min(block.buffer.len());
            block.buffer[offset.min(end)..end].fill(0);
        }
        let block_id = block.block_id;
        self.size_mismatch(block_id, packet_id, len, stride);
    }

    fn on_trailer(&mut self, index: usize, header: &GvspHeader, trailer: GvspTrailer) {
        let limit = self.buffer_limit();
        let block = &mut self.in_flight[index];
        if block.trailer.is_some() {
            self.stats.record_duplicate_packet();
            return;
        }
        let last_payload_id = header.packet_id.saturating_sub(1);
        block.trailer = Some(TrailerInfo {
            last_payload_id,
            size_y: trailer.size_y,
            payload_type: trailer.payload_type,
        });
        if block.unsupported {
            block.resolved = Some(Resolution::Unsupported);
            return;
        }
        if header.status & STATUS_ERROR != 0 {
            block.resolved = Some(Resolution::Frame(FrameStatus::DeviceStatus(header.status)));
            self.stats
                .record_missing_packets(missing_with_trailer(block) as u64);
            return;
        }
        // The trailer is remote input: track no more packets than the buffer
        // could hold, however many it claims. A claim beyond that can never
        // complete, and closes on the timeout like any other gap.
        let stride = payload_stride(self.config.packet_size, header.header_len).max(1);
        block
            .bitmap
            .grow((last_payload_id as usize).min(limit.div_ceil(stride)));
        if let Some(short) = block.short_packet
            && short.0 < last_payload_id
        {
            block.short_packet = None;
            self.refuse_short(index, short.0, short.1, short.2);
        }
    }

    fn try_complete(&mut self, index: usize) {
        let block = &mut self.in_flight[index];
        if block.resolved.is_some() || block.unsupported || block.leader.is_none() {
            return;
        }
        if let Some(trailer) = &block.trailer
            && block
                .bitmap
                .first_n_complete(trailer.last_payload_id as usize)
        {
            block.resolved = Some(Resolution::Frame(FrameStatus::Complete));
        }
    }

    /// Resolve an unfinished block with what it has.
    fn close(&mut self, index: usize) {
        let block = &mut self.in_flight[index];
        if block.unsupported {
            block.resolved = Some(Resolution::Unsupported);
            return;
        }
        let status = if block.trailer.is_some() {
            let missing = missing_with_trailer(block);
            self.stats.record_missing_packets(missing as u64);
            FrameStatus::Incomplete {
                missing_packets: missing,
            }
        } else {
            // Without a trailer the count is a lower bound: the holes below the
            // highest packet seen, the trailer itself and a lost leader.
            let holes = (0..block.highest_payload_id as usize)
                .filter(|slot| !block.bitmap.contains(*slot))
                .count() as u64;
            let leader = u64::from(block.leader_payload_type.is_none());
            self.stats.record_missing_packets(holes + 1 + leader);
            FrameStatus::TrailerMissing
        };
        debug!(
            block_id = block.block_id,
            ?status,
            "closing an unfinished block"
        );
        block.resolved = Some(Resolution::Frame(status));
    }

    /// Hand out resolved blocks from the front, in the order they started.
    fn emit(&mut self, out: &mut Vec<AssembledBlock>) {
        while self.in_flight.front().is_some_and(|b| b.resolved.is_some()) {
            let Some(mut block) = self.in_flight.pop_front() else {
                break;
            };
            let resolution = block.resolved.expect("checked above");
            self.spare_bitmaps.push(std::mem::take(&mut block.bitmap));
            let status = match resolution {
                Resolution::Unsupported => {
                    self.stats.record_unsupported_block();
                    continue;
                }
                Resolution::Frame(status) => status,
            };
            let mut buffer = std::mem::take(&mut block.buffer);
            buffer.truncate(block.extent);
            let (trailer_size_y, trailer_type) = match block.trailer.take() {
                Some(t) => (t.size_y, Some(t.payload_type)),
                None => (0, None),
            };
            out.push(AssembledBlock {
                block_id: block.block_id,
                status,
                leader: block.leader,
                payload_type: block.leader_payload_type.or(trailer_type).unwrap_or(0),
                payload: Bytes::from(buffer),
                trailer_size_y,
            });
        }
    }

    fn out_of_range(&mut self, index: usize, packet_id: u32, len: usize) {
        if !self.warned.out_of_range {
            self.warned.out_of_range = true;
            let block = &self.in_flight[index];
            warn!(
                block_id = block.block_id,
                packet_id,
                len,
                buffer = block.buffer.len(),
                payload_size = self.config.payload_size,
                "a GVSP data packet lies beyond the frame: either past the packet count the \
                 trailer gave, or past PayloadSize. If PayloadSize was read before a format or \
                 chunk change, read it again"
            );
        }
        self.stats.record_ignored_packet(IgnoredPacket::OutOfRange);
    }

    fn size_mismatch(&mut self, block_id: u64, packet_id: u32, len: usize, stride: usize) {
        if !self.warned.size_mismatch {
            self.warned.size_mismatch = true;
            warn!(
                block_id,
                packet_id,
                observed = len,
                expected = stride,
                packet_size = self.config.packet_size,
                "a GVSP data packet that is not the block's last carries {len} bytes where the \
                 packet size implies {stride}. The camera and the host disagree about \
                 GevSCPSPacketSize (SR-02); retry with an explicit packet size of 1500"
            );
        }
        self.stats.record_size_mismatch();
    }
}

/// Packets missing from a block whose trailer arrived: data packets not
/// received, plus the leader if it was lost. Bounded by the bitmap, not by the
/// count the trailer claims.
fn missing_with_trailer(block: &BlockInFlight) -> u32 {
    let last = block
        .trailer
        .as_ref()
        .map_or(0, |t| t.last_payload_id as usize);
    let tracked = last.min(block.bitmap.len());
    let holes = (0..tracked)
        .filter(|slot| !block.bitmap.contains(*slot))
        .count()
        + (last - tracked);
    let missing = holes + usize::from(block.leader_payload_type.is_none());
    u32::try_from(missing).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    //! Datagrams come from `viva_fake_gige::gvsp`, written from the GigE Vision
    //! field tables with the fake's own constants (ADR-0019); instants are
    //! invented, so nothing here sleeps.

    use super::*;
    use crate::stream::resend::ResendRequest;
    use viva_fake_gige::gvsp::{BlockSpec, Datagram, PacketKind, block_datagrams, image_size};
    use viva_fake_gige::{MONO8, MONO12_PACKED, PacketScript, ScriptStep};

    const PACKET_SIZE: u32 = 576;
    const MS: Duration = Duration::from_millis(1);

    fn image(len: usize, seed: u32) -> Vec<u8> {
        (0..len as u32)
            .map(|i| {
                i.wrapping_mul(2_654_435_761)
                    .wrapping_add(seed)
                    .rotate_right(13) as u8
            })
            .collect()
    }

    fn spec(block_id: u64, width: u32, height: u32) -> BlockSpec {
        BlockSpec::image(block_id, width, height, MONO8, PACKET_SIZE)
    }

    fn assembler() -> (Assembler, StreamStatsAccumulator) {
        let stats = StreamStatsAccumulator::new();
        (
            Assembler::new(AssemblyConfig::new(PACKET_SIZE), stats.clone()),
            stats,
        )
    }

    /// Feed every datagram at `at`, collecting what finishes.
    fn feed(asm: &mut Assembler, datagrams: &[Datagram], at: Instant) -> Vec<AssembledBlock> {
        let mut out = Vec::new();
        for d in datagrams {
            asm.ingest(&d.bytes, at, &mut out);
        }
        out
    }

    fn poll(asm: &mut Assembler, at: Instant) -> Vec<AssembledBlock> {
        let mut out = Vec::new();
        asm.poll(at, &mut out);
        out
    }

    #[test]
    fn an_ordered_block_completes_with_its_leader_fields() {
        let (mut asm, stats) = assembler();
        let img = image(64 * 32, 1);
        let mut s = spec(7, 64, 32);
        s.timestamp = 0xABCD;
        s.payload_type = 0x4001;
        let out = feed(&mut asm, &block_datagrams(&s, &img), Instant::now());
        assert_eq!(out.len(), 1);
        let block = &out[0];
        assert_eq!(block.status, FrameStatus::Complete);
        assert_eq!(block.block_id, 7);
        assert_eq!(block.payload.as_ref(), img.as_slice());
        assert_eq!(block.payload_type, 0x4001, "TC-05: the chunk bit survives");
        let leader = block.leader.expect("leader kept");
        assert_eq!(
            (leader.width, leader.height, leader.timestamp),
            (64, 32, 0xABCD)
        );
        assert_eq!(block.trailer_size_y, 32);
        assert_eq!(asm.in_flight(), 0);
        let snap = stats.snapshot();
        assert_eq!(
            snap.packets_ignored() + snap.packets_duplicate,
            0,
            "{snap:?}"
        );
    }

    /// The stride is `packet size − 28 − header`, with the header length read
    /// from each packet: 540 bytes at 576 with standard ids, 528 with extended.
    #[test]
    fn extended_ids_use_the_20_byte_header_stride() {
        let (mut asm, _) = assembler();
        let img = image(64 * 32, 2);
        let mut s = spec(0x0001_0000_0000_0001, 64, 32);
        s.extended_ids = true;
        let datagrams = block_datagrams(&s, &img);
        assert_eq!(datagrams[1].bytes.len(), 20 + 528);
        let out = feed(&mut asm, &datagrams, Instant::now());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].block_id, 0x0001_0000_0000_0001);
        assert_eq!(out[0].payload.as_ref(), img.as_slice());
    }

    #[test]
    fn an_early_trailer_only_records_the_packet_count() {
        let (mut asm, _) = assembler();
        let img = image(64 * 32, 3);
        let script = PacketScript::new().step(ScriptStep::TrailerBeforeLastPayload { block: 1 });
        let datagrams = script.apply(block_datagrams(&spec(1, 64, 32), &img));
        let (head, last) = datagrams.split_at(datagrams.len() - 1);
        let t0 = Instant::now();
        assert!(
            feed(&mut asm, head, t0).is_empty(),
            "the trailer alone must not close it"
        );
        let out = feed(&mut asm, last, t0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].status, FrameStatus::Complete);
        assert_eq!(out[0].payload.as_ref(), img.as_slice());
    }

    #[test]
    fn a_repeated_leader_is_a_duplicate_not_a_reset() {
        let (mut asm, stats) = assembler();
        let img = image(64 * 32, 4);
        let script = PacketScript::new().step(ScriptStep::RepeatLeader { block: 1, after: 2 });
        let out = feed(
            &mut asm,
            &script.apply(block_datagrams(&spec(1, 64, 32), &img)),
            Instant::now(),
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].payload.as_ref(), img.as_slice());
        assert_eq!(stats.snapshot().packets_duplicate, 1);
    }

    /// Inactivity, not age: a block whose packets keep coming completes however
    /// long it takes; one that falls silent closes one timeout after its last
    /// packet.
    #[test]
    fn the_timeout_counts_from_the_last_packet() {
        let (mut asm, stats) = assembler();
        let img = image(216 * 100, 5);
        let datagrams = block_datagrams(&spec(1, 216, 100), &img);
        assert_eq!(datagrams.len(), 42);
        let t0 = Instant::now();
        let mut out = Vec::new();
        for (i, d) in datagrams.iter().enumerate() {
            // 60 ms apart: 2.5 s from leader to trailer.
            let at = t0 + 60 * MS * i as u32;
            out.extend(poll(&mut asm, at));
            asm.ingest(&d.bytes, at, &mut out);
        }
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].status, FrameStatus::Complete);

        // Silence after packet 5 of the next block closes it 100 ms later.
        let next = block_datagrams(&spec(2, 216, 100), &img);
        let t1 = t0 + Duration::from_secs(10);
        assert!(feed(&mut asm, &next[..6], t1).is_empty());
        assert!(poll(&mut asm, t1 + 99 * MS).is_empty());
        let closed = poll(&mut asm, t1 + 100 * MS);
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].status, FrameStatus::TrailerMissing);
        // Five data packets arrived (1..=5): no holes below them, the trailer
        // is the only packet known to be missing.
        assert_eq!(stats.snapshot().packets_missing, 1);
    }

    #[test]
    fn a_shorter_timeout_applies_to_blocks_in_flight() {
        let (mut asm, _) = assembler();
        let datagrams = block_datagrams(&spec(1, 64, 32), &image(64 * 32, 6));
        let t0 = Instant::now();
        feed(&mut asm, &datagrams[..2], t0);
        asm.set_config(AssemblyConfig {
            block_timeout: 10 * MS,
            ..asm.config()
        });
        assert_eq!(poll(&mut asm, t0 + 10 * MS).len(), 1);
    }

    /// aravis does not time out the newest block while its leader is all it
    /// has: some devices send the leader early.
    #[test]
    fn a_leader_alone_does_not_time_out_while_it_is_the_newest() {
        let (mut asm, _) = assembler();
        let first = block_datagrams(&spec(1, 64, 32), &image(64 * 32, 7));
        let t0 = Instant::now();
        feed(&mut asm, &first[..1], t0);
        assert!(poll(&mut asm, t0 + Duration::from_secs(5)).is_empty());
        // A newer block makes it an ordinary block, which then times out.
        let second = block_datagrams(&spec(2, 64, 32), &image(64 * 32, 8));
        let t1 = t0 + Duration::from_secs(6);
        feed(&mut asm, &second[..1], t1);
        let closed = poll(&mut asm, t1);
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].block_id, 1);
        assert_eq!(closed[0].status, FrameStatus::TrailerMissing);
    }

    #[test]
    fn interleaved_blocks_complete_and_leave_in_order() {
        let (mut asm, _) = assembler();
        let a = image(64 * 32, 9);
        let b = image(64 * 32, 10);
        let merged = viva_fake_gige::gvsp::interleave(
            block_datagrams(&spec(1, 64, 32), &a),
            block_datagrams(&spec(2, 64, 32), &b),
        );
        let out = feed(&mut asm, &merged, Instant::now());
        let ids: Vec<u64> = out.iter().map(|b| b.block_id).collect();
        assert_eq!(ids, vec![1, 2]);
        assert_eq!(out[0].payload.as_ref(), a.as_slice());
        assert_eq!(out[1].payload.as_ref(), b.as_slice());
    }

    /// A block that completes while an older one is still open waits for it:
    /// frames leave in the order they started.
    #[test]
    fn a_complete_block_waits_for_an_older_open_one() {
        let (mut asm, _) = assembler();
        let older = block_datagrams(&spec(1, 64, 32), &image(64 * 32, 11));
        let newer = block_datagrams(&spec(2, 64, 32), &image(64 * 32, 12));
        let t0 = Instant::now();
        // Block 1 loses its trailer; block 2 arrives whole.
        assert!(feed(&mut asm, &older[..older.len() - 1], t0).is_empty());
        assert!(feed(&mut asm, &newer, t0).is_empty(), "held behind block 1");
        let out = poll(&mut asm, t0 + 100 * MS);
        let order: Vec<(u64, FrameStatus)> = out.iter().map(|b| (b.block_id, b.status)).collect();
        assert_eq!(
            order,
            vec![(1, FrameStatus::TrailerMissing), (2, FrameStatus::Complete)]
        );
    }

    #[test]
    fn a_fourth_block_closes_the_oldest() {
        let (mut asm, _) = assembler();
        let t0 = Instant::now();
        for id in 1..=3 {
            let d = block_datagrams(&spec(id, 64, 32), &image(64 * 32, id as u32));
            assert!(feed(&mut asm, &d[..2], t0).is_empty());
        }
        assert_eq!(asm.in_flight(), MAX_IN_FLIGHT);
        let d = block_datagrams(&spec(4, 64, 32), &image(64 * 32, 4));
        let out = feed(&mut asm, &d[..1], t0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].block_id, 1);
        assert_eq!(out[0].status, FrameStatus::TrailerMissing);
        assert_eq!(asm.in_flight(), MAX_IN_FLIGHT);
    }

    #[test]
    fn sixteen_bit_ids_wrap_past_zero() {
        assert_eq!(distance16(65535, 1), 1);
        assert_eq!(distance16(65534, 1), 2);
        assert_eq!(distance16(1, 65535), -1);
        assert_eq!(distance16(10, 7), -3);
        assert_eq!(distance16(100, 200), 100);
        assert_eq!(
            classify(Some(65535), 1, false, true),
            Order::Next { missed: 0 }
        );
        assert_eq!(
            classify(Some(65534), 2, false, true),
            Order::Next { missed: 2 }
        );
        assert_eq!(classify(Some(500), 450, false, true), Order::Late);
        assert_eq!(classify(Some(500), 300, false, true), Order::Restart);
        // Back to 1 is a restart, however near or far.
        assert_eq!(classify(Some(50), 1, false, true), Order::Restart);
        assert_eq!(classify(Some(40_000), 1, false, true), Order::Restart);
        assert_eq!(classify(None, 9, false, true), Order::Unordered);
        assert_eq!(classify(Some(9), 0, false, true), Order::Unordered);
        // A data packet, or a repeat, of an old block 1 is late, not a restart.
        assert_eq!(classify(Some(50), 1, false, false), Order::Late);
        assert_eq!(classify(Some(1), 1, false, true), Order::Late);
    }

    #[test]
    fn sixty_four_bit_ids_count_gaps_without_wrapping() {
        assert_eq!(
            classify(Some(1 << 40), (1 << 40) + 3, true, true),
            Order::Next { missed: 2 }
        );
        assert_eq!(
            classify(Some(1 << 40), (1 << 40) - 5, true, true),
            Order::Late
        );
        assert_eq!(classify(Some(1 << 40), 1, true, true), Order::Restart);
    }

    #[test]
    fn skipped_block_ids_count_as_missed_and_old_ones_as_late() {
        let (mut asm, stats) = assembler();
        let t0 = Instant::now();
        let mut out = Vec::new();
        for id in [65533u64, 65534, 65535, 1, 3, 7] {
            out.extend(feed(
                &mut asm,
                &block_datagrams(&spec(id, 32, 32), &image(1024, 1)),
                t0,
            ));
        }
        assert_eq!(out.len(), 6);
        assert_eq!(stats.snapshot().frames_missed, 1 + 3);
        // Block 2 turns up after 7: late, not a new block.
        feed(
            &mut asm,
            &block_datagrams(&spec(2, 32, 32), &image(1024, 1)),
            t0,
        );
        let snap = stats.snapshot();
        assert_eq!(snap.packets_ignored_late, 4, "{snap:?}");
        assert_eq!(asm.in_flight(), 0);
    }

    /// `PayloadSize` sizes the buffer; a stale one smaller than the frame —
    /// read before Width grew — does not fail the frame.
    #[test]
    fn a_stale_payload_size_grows_rather_than_fails() {
        let stats = StreamStatsAccumulator::new();
        let mut config = AssemblyConfig::new(PACKET_SIZE);
        config.payload_size = Some(1000);
        let mut asm = Assembler::new(config, stats.clone());
        let img = image(2048, 13);
        let out = feed(
            &mut asm,
            &block_datagrams(&spec(1, 64, 32), &img),
            Instant::now(),
        );
        assert_eq!(out[0].status, FrameStatus::Complete);
        assert_eq!(out[0].payload.as_ref(), img.as_slice());
        assert_eq!(stats.snapshot().packets_ignored_out_of_range, 0);
    }

    /// A data packet beyond the count the trailer gave is out of range.
    #[test]
    fn a_packet_beyond_the_trailer_count_is_out_of_range() {
        let (mut asm, stats) = assembler();
        let datagrams = block_datagrams(&spec(1, 64, 32), &image(2048, 22));
        let mut extra = datagrams[1].clone();
        // Packet id 9 in a block whose trailer says 4 data packets.
        extra.bytes[7] = 9;
        let t0 = Instant::now();
        let mut order = datagrams.clone();
        order.push(extra);
        let out = feed(&mut asm, &order, t0);
        assert_eq!(out.len(), 1);
        assert_eq!(
            stats.snapshot().packets_ignored_late,
            1,
            "after the block left"
        );

        let (mut asm, stats) = assembler();
        let mut early = datagrams[..datagrams.len() - 1].to_vec();
        let trailer = datagrams.last().unwrap().clone();
        early.retain(|d| d.packet_id != 2);
        early.push(trailer);
        let mut extra = datagrams[1].clone();
        extra.bytes[7] = 9;
        early.push(extra);
        assert!(feed(&mut asm, &early, t0).is_empty());
        assert_eq!(stats.snapshot().packets_ignored_out_of_range, 1);
    }

    /// `PayloadSize` is a ceiling, not a promise: an EVT block or a shorter
    /// frame delivers what was sent, not the whole buffer.
    #[test]
    fn a_block_smaller_than_payload_size_is_delivered_at_its_own_length() {
        let stats = StreamStatsAccumulator::new();
        let mut config = AssemblyConfig::new(PACKET_SIZE);
        config.payload_size = Some(1 << 20);
        let mut asm = Assembler::new(config, stats);
        let img = image(824, 14);
        let out = feed(
            &mut asm,
            &block_datagrams(&spec(1, 1, 824), &img),
            Instant::now(),
        );
        assert_eq!(out[0].payload.as_ref(), img.as_slice());
    }

    /// The buffer is handed in once, at `PayloadSize`, and written in place.
    #[test]
    fn the_buffer_is_acquired_once_per_block_at_payload_size() {
        #[derive(Clone, Default)]
        struct Recording(std::sync::Arc<std::sync::Mutex<Vec<usize>>>);
        impl BufferSource for Recording {
            fn acquire(&mut self, len: usize) -> Vec<u8> {
                self.0.lock().unwrap().push(len);
                vec![0; len]
            }
        }
        let source = Recording::default();
        let mut config = AssemblyConfig::new(PACKET_SIZE);
        config.payload_size = Some(64 * 32);
        let mut asm = Assembler::with_buffers(
            config,
            StreamStatsAccumulator::new(),
            Box::new(source.clone()),
        );
        let t0 = Instant::now();
        for id in 1..=3 {
            feed(
                &mut asm,
                &block_datagrams(&spec(id, 64, 32), &image(2048, 1)),
                t0,
            );
        }
        assert_eq!(*source.0.lock().unwrap(), vec![2048, 2048, 2048]);
    }

    /// Without `PayloadSize`, a packed format is sized from its bit depth, not
    /// a whole number of bytes a pixel.
    #[test]
    fn a_packed_leader_sizes_the_buffer_by_bit_depth() {
        let (mut asm, _) = assembler();
        let size = image_size(MONO12_PACKED, 64, 32);
        let img = image(size, 15);
        let s = BlockSpec::image(1, 64, 32, MONO12_PACKED, PACKET_SIZE);
        let out = feed(&mut asm, &block_datagrams(&s, &img), Instant::now());
        assert_eq!(out[0].payload.len(), 3072);
        assert_eq!(out[0].payload.as_ref(), img.as_slice());
        assert_eq!(asm.frame_size_hint(), Some(3072));
    }

    #[test]
    fn a_leader_cannot_allocate_more_than_the_cap() {
        let (mut asm, _) = assembler();
        let s = spec(1, u32::MAX, u32::MAX);
        feed(&mut asm, &block_datagrams(&s, &[])[..1], Instant::now());
        assert_eq!(asm.frame_size_hint(), Some(ESTIMATE_CAP));
    }

    /// A data packet longer than the stride would overlap its neighbour; it is
    /// refused and counted, so the frame is visibly incomplete instead of
    /// silently scrambled. A short packet is legitimate only as the last.
    #[test]
    fn packets_off_the_stride_are_counted() {
        let (mut asm, stats) = assembler();
        // The camera believes 600, we believe 576: every full packet is long.
        let mut s = spec(1, 64, 32);
        s.packet_size = 600;
        let out = feed(
            &mut asm,
            &block_datagrams(&s, &image(2048, 16)),
            Instant::now(),
        );
        assert!(out.is_empty());
        assert_eq!(stats.snapshot().packets_size_mismatch, 3);

        // The camera believes 500: full packets are short, and packet 1 is
        // shown not to be the last once packet 2 arrives.
        let (mut asm, stats) = assembler();
        let mut s = spec(1, 64, 32);
        s.packet_size = 500;
        feed(
            &mut asm,
            &block_datagrams(&s, &image(2048, 17)),
            Instant::now(),
        );
        // Packets 1..=4 are short, 5 is the last: four mismatches.
        assert_eq!(stats.snapshot().packets_size_mismatch, 4);
    }

    /// A short packet that is not the last is taken back out: the frame closes
    /// incomplete, with zeros where it was, instead of completing with shifted
    /// data (Codex review of #194).
    #[test]
    fn a_frame_of_short_packets_is_not_complete() {
        let (mut asm, stats) = assembler();
        let mut s = spec(1, 64, 32);
        s.packet_size = 500;
        let t0 = Instant::now();
        let mut out = feed(&mut asm, &block_datagrams(&s, &image(2048, 17)), t0);
        out.extend(poll(&mut asm, t0 + Duration::from_secs(1)));
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].status,
            FrameStatus::Incomplete { missing_packets: 4 },
            "packets 1..=4 are short and not the last"
        );
        assert!(out[0].payload[..540 * 4].iter().all(|b| *b == 0));
        assert_eq!(stats.snapshot().packets_size_mismatch, 4);
    }

    fn resend_assembler() -> (Assembler, StreamStatsAccumulator) {
        let stats = StreamStatsAccumulator::new();
        let mut config = AssemblyConfig::new(PACKET_SIZE);
        config.resend.enabled = true;
        (Assembler::new(config, stats.clone()), stats)
    }

    fn due(asm: &mut Assembler, at: Instant) -> Vec<ResendRequest> {
        let mut out = Vec::new();
        asm.resend_due(at, &mut out);
        out
    }

    /// A lost data packet is asked for once its gap is old enough, and
    /// counts as recovered when it comes.
    #[test]
    fn a_lost_packet_is_asked_for_and_counted_when_it_comes() {
        let (mut asm, stats) = resend_assembler();
        let datagrams = block_datagrams(&spec(5, 64, 32), &image(2048, 30));
        let t0 = Instant::now();
        let lost = datagrams[2].clone();
        let rest: Vec<Datagram> = datagrams
            .iter()
            .filter(|d| d.packet_id != 2)
            .cloned()
            .collect();
        assert!(feed(&mut asm, &rest, t0).is_empty());
        assert!(due(&mut asm, t0 + MS).is_empty(), "younger than the delay");
        assert_eq!(
            due(&mut asm, t0 + 2 * MS),
            vec![ResendRequest {
                block_id: 5,
                first: 2,
                last: 2,
                extended_ids: false
            }]
        );
        let out = feed(&mut asm, &[lost], t0 + 3 * MS);
        assert_eq!(out[0].status, FrameStatus::Complete);
        let snap = stats.snapshot();
        assert_eq!((snap.resend_requested, snap.resend_recovered), (1, 1));
    }

    /// A packet the camera sends again may carry status `0x0100`
    /// (`PACKET_RESEND`): a success, so it is used.
    #[test]
    fn a_packet_marked_as_resent_is_used() {
        let (mut asm, _) = assembler();
        let mut datagrams = block_datagrams(&spec(1, 64, 32), &image(2048, 31));
        datagrams[2].bytes[0] = 0x01;
        let out = feed(&mut asm, &datagrams, Instant::now());
        assert_eq!(out[0].status, FrameStatus::Complete);
    }

    /// The camera's "no longer available" is a bare header, here in the
    /// trailer's format. It closes the range and is not the frame's status.
    #[test]
    fn an_unavailable_answer_closes_the_range_not_the_frame() {
        let (mut asm, stats) = resend_assembler();
        let datagrams = block_datagrams(&spec(5, 64, 32), &image(2048, 32));
        let t0 = Instant::now();
        let rest: Vec<Datagram> = datagrams
            .iter()
            .filter(|d| d.packet_id != 2)
            .cloned()
            .collect();
        feed(&mut asm, &rest, t0);
        assert_eq!(due(&mut asm, t0 + 2 * MS).len(), 1);
        let bare = Datagram {
            block_id: 5,
            packet_id: 2,
            kind: PacketKind::Trailer,
            bytes: vec![0x80, 0x0C, 0x00, 0x05, 0x02, 0x00, 0x00, 0x02],
        };
        assert!(feed(&mut asm, &[bare], t0 + 3 * MS).is_empty());
        assert!(due(&mut asm, t0 + 30 * MS).is_empty(), "not asked again");
        let out = poll(&mut asm, t0 + Duration::from_secs(1));
        assert_eq!(
            out[0].status,
            FrameStatus::Incomplete { missing_packets: 1 }
        );
        let snap = stats.snapshot();
        assert_eq!(snap.resend_unavailable, 1);
        assert_eq!(snap.packets_ignored_error_status, 1);
    }

    /// "Not yet available" and "temporarily unavailable" leave the range to
    /// its retries.
    #[test]
    fn a_transient_answer_leaves_the_range_to_its_retries() {
        let (mut asm, stats) = resend_assembler();
        let datagrams = block_datagrams(&spec(5, 64, 32), &image(2048, 33));
        let t0 = Instant::now();
        let rest: Vec<Datagram> = datagrams
            .iter()
            .filter(|d| d.packet_id != 2)
            .cloned()
            .collect();
        feed(&mut asm, &rest, t0);
        assert_eq!(due(&mut asm, t0 + 2 * MS).len(), 1);
        let bare = Datagram {
            block_id: 5,
            packet_id: 2,
            kind: PacketKind::Payload,
            bytes: vec![0x80, 0x14, 0x00, 0x05, 0x03, 0x00, 0x00, 0x02],
        };
        feed(&mut asm, &[bare], t0 + 3 * MS);
        assert_eq!(due(&mut asm, t0 + 22 * MS).len(), 1, "asked again");
        assert_eq!(stats.snapshot().resend_unavailable, 0);
    }

    /// A short packet is not asked for again: the camera would send the same
    /// bytes.
    #[test]
    fn a_short_packet_is_not_asked_for_again() {
        let (mut asm, stats) = resend_assembler();
        let mut s = spec(1, 64, 32);
        s.packet_size = 500;
        let t0 = Instant::now();
        feed(&mut asm, &block_datagrams(&s, &image(2048, 34)), t0);
        assert!(due(&mut asm, t0 + 10 * MS).is_empty());
        assert_eq!(stats.snapshot().resend_requested, 0);
    }

    /// A trailer claiming four billion packets allocates and counts nothing on
    /// that say-so; the block closes incomplete on the timeout.
    #[test]
    fn an_absurd_trailer_packet_count_is_bounded() {
        let (mut asm, stats) = assembler();
        let mut s = spec(1, 64, 32);
        s.extended_ids = true;
        let mut datagrams = block_datagrams(&s, &image(2048, 21));
        let trailer = datagrams.last_mut().unwrap();
        trailer.bytes[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
        let t0 = Instant::now();
        assert!(feed(&mut asm, &datagrams, t0).is_empty());
        let out = poll(&mut asm, t0 + Duration::from_secs(1));
        assert!(matches!(out[0].status, FrameStatus::Incomplete { .. }));
        assert!(stats.snapshot().packets_missing > 1 << 31);
    }

    #[test]
    fn a_trailer_error_status_is_reported_as_the_frame_status() {
        let (mut asm, _) = assembler();
        let mut s = spec(1, 64, 32);
        s.trailer_status = 0x8015;
        let out = feed(
            &mut asm,
            &block_datagrams(&s, &image(2048, 18)),
            Instant::now(),
        );
        assert_eq!(out[0].status, FrameStatus::DeviceStatus(0x8015));
    }

    #[test]
    fn a_data_packet_with_an_error_status_is_ignored() {
        let (mut asm, stats) = assembler();
        let mut datagrams = block_datagrams(&spec(1, 64, 32), &image(2048, 19));
        datagrams[2].bytes[0] = 0x80;
        datagrams[2].bytes[1] = 0x0C; // PACKET_UNAVAILABLE
        let out = feed(&mut asm, &datagrams, Instant::now());
        assert!(out.is_empty());
        assert_eq!(stats.snapshot().packets_ignored_error_status, 1);
        let out = poll(&mut asm, Instant::now() + Duration::from_secs(1));
        assert_eq!(
            out[0].status,
            FrameStatus::Incomplete { missing_packets: 1 }
        );
    }

    /// TC-05: a block of another payload type is counted, not silently lost.
    #[test]
    fn an_unsupported_payload_type_is_counted() {
        let (mut asm, stats) = assembler();
        let mut s = spec(1, 64, 32);
        s.payload_type = 0x0003; // file
        let out = feed(
            &mut asm,
            &block_datagrams(&s, &image(2048, 20)),
            Instant::now(),
        );
        assert!(out.is_empty());
        let snap = stats.snapshot();
        assert_eq!(snap.frames_unsupported, 1);
        // The leader and four data packets; the trailer closes the block.
        assert_eq!(snap.packets_ignored_unsupported, 5, "{snap:?}");
    }

    #[test]
    fn a_datagram_that_is_not_gvsp_is_counted() {
        let (mut asm, stats) = assembler();
        feed(
            &mut asm,
            &[Datagram {
                block_id: 0,
                packet_id: 0,
                kind: PacketKind::Payload,
                bytes: vec![1, 2, 3],
            }],
            Instant::now(),
        );
        assert_eq!(stats.snapshot().packets_ignored_malformed, 1);
    }

    /// Property: any permutation of a block's datagrams, with repeats, yields
    /// the original bytes; with losses, an incomplete block whose received
    /// bytes are still in place. Seeded, so a failure names its case.
    #[test]
    fn any_order_with_repeats_and_losses_reassembles_what_arrived() {
        for seed in 0..400u64 {
            let mut rng = fastrand::Rng::with_seed(seed);
            let width = rng.u32(1..200);
            let height = rng.u32(1..24);
            let packet_size = rng.u32(64..700);
            let extended_ids = rng.bool();
            let mut s = BlockSpec::image(rng.u64(1..1 << 40), width, height, MONO8, packet_size);
            s.extended_ids = extended_ids;
            if !extended_ids {
                s.block_id = rng.u64(1..65536);
            }
            let stride = s.payload_stride();
            let img = image((width * height) as usize, seed as u32);
            let mut datagrams = block_datagrams(&s, &img);

            let lose = rng.u8(..4) == 0;
            let mut lost: Vec<u32> = Vec::new();
            if lose {
                let victim = rng.usize(..datagrams.len());
                lost.push(datagrams[victim].packet_id);
                datagrams.remove(victim);
            }
            let repeats = rng.usize(..4);
            for _ in 0..repeats {
                let copy = datagrams[rng.usize(..datagrams.len())].clone();
                datagrams.push(copy);
            }
            rng.shuffle(&mut datagrams);

            let stats = StreamStatsAccumulator::new();
            let mut asm = Assembler::new(AssemblyConfig::new(packet_size), stats.clone());
            let t0 = Instant::now();
            let mut out = feed(&mut asm, &datagrams, t0);
            out.extend(poll(&mut asm, t0 + Duration::from_secs(1)));
            let case = format!(
                "seed {seed}: {width}x{height} at {packet_size}, extended {extended_ids}, lost {lost:?}"
            );
            assert_eq!(out.len(), 1, "{case}");
            let block = &out[0];
            let snap = stats.snapshot();
            assert_eq!(
                snap.packets_duplicate + snap.packets_ignored_late,
                repeats as u64,
                "{case}: every repeat is counted once"
            );
            if lost.is_empty() {
                assert_eq!(block.status, FrameStatus::Complete, "{case}");
                assert!(block.payload.as_ref() == img.as_slice(), "{case}");
                continue;
            }
            assert!(!block.status.is_complete(), "{case}: {:?}", block.status);
            for d in datagrams.iter().filter(|d| d.kind == PacketKind::Payload) {
                let start = (d.packet_id as usize - 1) * stride;
                let end = (start + stride).min(img.len());
                assert_eq!(&block.payload[start..end], &img[start..end], "{case}");
            }
        }
    }

    /// The same, across several blocks at once: whatever the interleaving,
    /// every block leaves once, in order, intact.
    #[test]
    fn shuffled_neighbouring_blocks_all_complete_in_order() {
        for seed in 0..100u64 {
            let mut rng = fastrand::Rng::with_seed(seed);
            let first = rng.u64(1..65000);
            let images: Vec<Vec<u8>> = (0..3)
                .map(|i| image(40 * 30, (seed * 3 + i) as u32))
                .collect();
            let mut datagrams: Vec<Datagram> = (0..3)
                .flat_map(|i| block_datagrams(&spec(first + i as u64, 40, 30), &images[i]))
                .collect();
            // Shuffle within a window, so the first packet of each block still
            // precedes the others' — as on a wire, where reordering is local.
            for chunk in datagrams.chunks_mut(4) {
                rng.shuffle(chunk);
            }
            let (mut asm, _) = assembler();
            let t0 = Instant::now();
            let mut out = feed(&mut asm, &datagrams, t0);
            out.extend(poll(&mut asm, t0 + Duration::from_secs(1)));
            let ids: Vec<u64> = out.iter().map(|b| b.block_id).collect();
            let complete = out.iter().filter(|b| b.status.is_complete()).count();
            assert_eq!(complete, out.len(), "seed {seed}: {ids:?}");
            for b in &out {
                let i = (b.block_id - first) as usize;
                assert!(b.payload.as_ref() == images[i].as_slice(), "seed {seed}");
            }
            assert!(ids.windows(2).all(|w| w[0] < w[1]), "seed {seed}: {ids:?}");
        }
    }
}
