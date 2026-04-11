//! GVSP packet parsing, reassembly and resend orchestration.
//!
//! The GigE Vision Streaming Protocol delivers image data over UDP. Packets can
//! arrive out of order or be dropped entirely; this module reconstructs complete
//! frames while coordinating resend requests over GVCP. The implementation keeps
//! copies to a minimum by writing directly into pooled [`BytesMut`] buffers that
//! are subsequently frozen into [`Bytes`] once a frame is ready.

use std::collections::VecDeque;
use std::net::Ipv4Addr;
use std::ops::RangeInclusive;
use std::time::{Duration, Instant};

use crate::nic::Iface;
use crate::stats::StreamStatsAccumulator;
use bytes::{Buf, Bytes, BytesMut};
use thiserror::Error;
use tracing::{debug, warn};

/// GVSP payload type for image data as defined by the specification (Table
/// 36). Other payload types are currently not supported by the reassembler but
/// will still be parsed.
const PAYLOAD_TYPE_IMAGE: u8 = 0x01;

/// Size of the GVSP header preceding payload packets. The reassembler uses the
/// value when allocating buffers so the application payload fits within the
/// negotiated packet size.
const GVSP_HEADER_SIZE: usize = 8;

/// Destination for GVSP packets received by the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamDest {
    /// Standard unicast delivery towards a single host.
    Unicast {
        /// Destination IPv4 address configured on the camera.
        dst_ip: Ipv4Addr,
        /// UDP port used for streaming.
        dst_port: u16,
    },
    /// Multicast delivery towards one or more hosts joined to the group.
    Multicast {
        /// Multicast group IPv4 address.
        group: Ipv4Addr,
        /// UDP port used for streaming.
        port: u16,
        /// Whether loopback is enabled on the local socket.
        loopback: bool,
        /// Outbound multicast time-to-live.
        ttl: u32,
    },
}

impl StreamDest {
    /// Retrieve the configured UDP port.
    pub fn port(&self) -> u16 {
        match self {
            StreamDest::Unicast { dst_port, .. } => *dst_port,
            StreamDest::Multicast { port, .. } => *port,
        }
    }

    /// Retrieve the configured IPv4 destination address.
    pub fn addr(&self) -> Ipv4Addr {
        match self {
            StreamDest::Unicast { dst_ip, .. } => *dst_ip,
            StreamDest::Multicast { group, .. } => *group,
        }
    }

    /// Whether the destination represents multicast delivery.
    pub fn is_multicast(&self) -> bool {
        matches!(self, StreamDest::Multicast { .. })
    }
}

/// Stream configuration shared between the control plane and GVSP receiver.
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Destination configuration for the GVSP stream.
    pub dest: StreamDest,
    /// Interface used for receiving packets and multicast subscription.
    pub iface: Iface,
    /// Override for GVSP packet size determined via control plane.
    pub packet_size: Option<u32>,
    /// Override for GVSP packet delay determined via control plane.
    pub packet_delay: Option<u32>,
    /// Optional source filter restricting packets to the configured IPv4 address.
    pub source_filter: Option<Ipv4Addr>,
    /// Whether GVCP resend requests should be issued when drops are detected.
    pub resend_enabled: bool,
}

/// Errors raised while handling GVSP packets.
#[derive(Debug, Error)]
pub enum GvspError {
    #[error("unsupported packet type: {0}")]
    Unsupported(&'static str),
    #[error("invalid packet: {0}")]
    Invalid(&'static str),
    #[error("resend timeout")]
    ResendTimeout,
}

/// Raw GVSP chunk extracted from a payload or trailer block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkRaw {
    pub id: u16,
    pub data: Bytes,
}

/// Parse a chunk payload following the `[id][reserved][length][data...]` layout.
pub fn parse_chunks(mut payload: &[u8]) -> Vec<ChunkRaw> {
    let mut chunks = Vec::new();
    while !payload.is_empty() {
        if payload.len() < 8 {
            warn!(remaining = payload.len(), "chunk header truncated");
            break;
        }
        let mut cursor = payload;
        let id = cursor.get_u16();
        let _reserved = cursor.get_u16();
        let length = cursor.get_u32() as usize;
        let total = 8 + length;
        if payload.len() < total {
            warn!(
                chunk_id = format_args!("{:#06x}", id),
                len = payload.len(),
                expected = total,
                "chunk data truncated"
            );
            break;
        }
        let data = Bytes::copy_from_slice(&payload[8..total]);
        debug!(
            chunk_id = format_args!("{:#06x}", id),
            len = length,
            "parsed chunk"
        );
        chunks.push(ChunkRaw { id, data });
        payload = &payload[total..];
    }
    chunks
}

/// Representation of a GVSP packet.
///
/// Block IDs are `u64` and packet IDs are `u32` to support both standard
/// (16-bit block / 24-bit packet) and extended ID mode (64-bit block /
/// 32-bit packet) as defined in GigE Vision 2.0+.
#[derive(Debug, Clone)]
pub enum GvspPacket {
    /// Start-of-frame leader packet with metadata.
    Leader {
        block_id: u64,
        packet_id: u32,
        payload_type: u8,
        timestamp: u64,
        width: u32,
        height: u32,
        pixel_format: u32,
    },
    /// Payload data packet carrying pixel bytes.
    Payload {
        block_id: u64,
        packet_id: u32,
        data: Bytes,
    },
    /// End-of-frame trailer packet.
    Trailer {
        block_id: u64,
        packet_id: u32,
        status: u16,
        chunk_data: Bytes,
    },
}

/// Parse a raw UDP payload into a GVSP packet.
/// Parse a GVSP packet from raw bytes.
///
/// GVSP header layout (8 bytes):
///
/// | Offset | Size | Field         |
/// |--------|------|---------------|
/// |      0 |    2 | Status        |
/// |      2 |    2 | Block ID      |
/// |      4 |    1 | Packet format |
/// |      5 |    3 | Packet ID     |
/// Size of the extended GVSP header (GigE Vision 2.0+).
const GVSP_EXTENDED_HEADER_SIZE: usize = 20;

/// Extended ID flag: bit 7 of the packet_format byte.
const EXTENDED_ID_FLAG: u8 = 0x80;

pub fn parse_packet(payload: &[u8]) -> Result<GvspPacket, GvspError> {
    if payload.len() < GVSP_HEADER_SIZE {
        return Err(GvspError::Invalid("GVSP header truncated"));
    }

    let packet_format_byte = payload[4];
    let extended = (packet_format_byte & EXTENDED_ID_FLAG) != 0;
    let packet_format = packet_format_byte & 0x0F;

    let (block_id, packet_id, data_offset) = if extended {
        // Extended ID header (20 bytes):
        // [0-1]  status
        // [2-3]  block_id low 16 (backward compat)
        // [4]    packet_format | 0x80
        // [5-7]  packet_id low 24
        // [8-15] block_id 64-bit
        // [16-19] packet_id 32-bit
        if payload.len() < GVSP_EXTENDED_HEADER_SIZE {
            return Err(GvspError::Invalid("extended GVSP header truncated"));
        }
        let block_id = u64::from_be_bytes([
            payload[8],
            payload[9],
            payload[10],
            payload[11],
            payload[12],
            payload[13],
            payload[14],
            payload[15],
        ]);
        let packet_id = u32::from_be_bytes([payload[16], payload[17], payload[18], payload[19]]);
        (block_id, packet_id, GVSP_EXTENDED_HEADER_SIZE)
    } else {
        // Standard header (8 bytes)
        let block_id = u16::from_be_bytes([payload[2], payload[3]]) as u64;
        let packet_id = u32::from_be_bytes([0, payload[5], payload[6], payload[7]]);
        (block_id, packet_id, GVSP_HEADER_SIZE)
    };

    let payload_type = (u16::from_be_bytes([payload[0], payload[1]]) >> 4) as u8;

    match packet_format {
        0x01 => parse_leader(packet_id, block_id, payload_type, &payload[data_offset..]),
        0x03 => parse_payload(packet_id, block_id, &payload[data_offset..]),
        0x02 => parse_trailer(packet_id, block_id, &payload[data_offset..]),
        _ => Err(GvspError::Unsupported("packet format")),
    }
}

/// Parse a GVSP Data Leader packet.
///
/// Leader payload layout:
///
/// | Offset | Size | Field        |
/// |--------|------|--------------|
/// |      0 |    2 | Reserved     |
/// |      2 |    2 | Payload type |
/// |      4 |    8 | Timestamp    |
/// |     12 |    4 | Pixel format |
/// |     16 |    4 | Width        |
/// |     20 |    4 | Height       |
fn parse_leader(
    packet_id: u32,
    block_id: u64,
    _payload_type_header: u8,
    payload: &[u8],
) -> Result<GvspPacket, GvspError> {
    if payload.len() < 24 {
        return Err(GvspError::Invalid("leader payload truncated"));
    }
    let mut cursor = payload;
    let _reserved = cursor.get_u16();
    let payload_type = cursor.get_u16() as u8;
    if payload_type != PAYLOAD_TYPE_IMAGE {
        return Err(GvspError::Unsupported("payload type"));
    }
    let timestamp = cursor.get_u64();
    let pixel_format = cursor.get_u32();
    let width = cursor.get_u32();
    let height = cursor.get_u32();
    Ok(GvspPacket::Leader {
        block_id,
        packet_id,
        payload_type,
        timestamp,
        width,
        height,
        pixel_format,
    })
}

fn parse_payload(packet_id: u32, block_id: u64, payload: &[u8]) -> Result<GvspPacket, GvspError> {
    Ok(GvspPacket::Payload {
        block_id,
        packet_id,
        data: Bytes::copy_from_slice(payload),
    })
}

fn parse_trailer(packet_id: u32, block_id: u64, payload: &[u8]) -> Result<GvspPacket, GvspError> {
    if payload.len() < 2 {
        return Err(GvspError::Invalid("trailer truncated"));
    }
    let mut cursor = payload;
    let status = cursor.get_u16();
    let chunk_data = if payload.len() > 2 {
        Bytes::copy_from_slice(&payload[2..])
    } else {
        Bytes::new()
    };
    Ok(GvspPacket::Trailer {
        block_id,
        packet_id,
        status,
        chunk_data,
    })
}

/// Bitmap tracking received packets within a block.
#[derive(Debug, Clone)]
pub struct PacketBitmap {
    words: Vec<u64>,
    received: usize,
    total: usize,
}

impl PacketBitmap {
    /// Create a bitmap with the given packet capacity.
    pub fn new(total: usize) -> Self {
        let words = total.div_ceil(64);
        Self {
            words: vec![0; words],
            received: 0,
            total,
        }
    }

    fn mask_for(&self, packet_id: usize) -> (usize, u64) {
        let word = packet_id / 64;
        let bit = packet_id % 64;
        (word, 1u64 << bit)
    }

    /// Mark a packet index as received.
    pub fn set(&mut self, packet_id: usize) -> bool {
        if packet_id >= self.total {
            return false;
        }
        let (word, mask) = self.mask_for(packet_id);
        let entry = &mut self.words[word];
        if *entry & mask == 0 {
            *entry |= mask;
            self.received += 1;
            true
        } else {
            false
        }
    }

    /// Check whether the bitmap reports all packets received.
    pub fn is_complete(&self) -> bool {
        self.received == self.total
    }

    /// Return missing packet ranges as inclusive `[start, end]` indices.
    pub fn missing_ranges(&self) -> Vec<RangeInclusive<u32>> {
        let mut ranges = Vec::new();
        let mut current: Option<(u32, u32)> = None;
        for idx in 0..self.total {
            let (word, mask) = self.mask_for(idx);
            let present = (self.words[word] & mask) != 0;
            match (present, current) {
                (false, None) => current = Some((idx as u32, idx as u32)),
                (false, Some((start, _))) => current = Some((start, idx as u32)),
                (true, Some((start, end))) => {
                    ranges.push(start..=end);
                    current = None;
                }
                _ => {}
            }
        }
        if let Some((start, end)) = current {
            ranges.push(start..=end);
        }
        ranges
    }
}

/// Representation of a partially received frame.
#[derive(Debug)]
pub struct FrameAssembly {
    block_id: u64,
    expected_packets: usize,
    packet_payload: usize,
    bitmap: PacketBitmap,
    buffer: BytesMut,
    lengths: Vec<usize>,
    deadline: Instant,
}

impl FrameAssembly {
    /// Create a new frame assembly using the supplied buffer.
    pub fn new(
        block_id: u64,
        expected_packets: usize,
        packet_payload: usize,
        buffer: BytesMut,
        deadline: Instant,
    ) -> Self {
        Self {
            block_id,
            expected_packets,
            packet_payload,
            bitmap: PacketBitmap::new(expected_packets),
            buffer,
            lengths: vec![0; expected_packets],
            deadline,
        }
    }

    /// Returns the block identifier associated with this frame.
    pub fn block_id(&self) -> u64 {
        self.block_id
    }

    /// Whether the reassembly deadline has elapsed.
    pub fn is_expired(&self, now: Instant) -> bool {
        now >= self.deadline
    }

    /// Insert a packet payload into the buffer.
    pub fn ingest(&mut self, packet_id: usize, payload: &[u8]) -> bool {
        if packet_id >= self.expected_packets || payload.len() > self.packet_payload {
            return false;
        }
        if !self.bitmap.set(packet_id) {
            return true;
        }
        // Track actual payload length for compaction at finish time.
        self.lengths[packet_id] = payload.len();
        let offset = packet_id * self.packet_payload;
        if self.buffer.len() < offset + payload.len() {
            self.buffer.resize(offset + payload.len(), 0);
        }
        self.buffer[offset..offset + payload.len()].copy_from_slice(payload);
        true
    }

    /// Finalise the frame if all packets have been received.
    pub fn finish(self) -> Option<Bytes> {
        if !self.bitmap.is_complete() {
            return None;
        }

        // If all packets except possibly the last are full-sized, we can
        // return a slice of the existing buffer without extra copying.
        let full_sized_prefix = if self.expected_packets > 0 {
            self.lengths
                .iter()
                .take(self.expected_packets.saturating_sub(1))
                .all(|&len| len == self.packet_payload)
        } else {
            true
        };

        if full_sized_prefix {
            let last_len = *self.lengths.last().unwrap_or(&0);
            let used = self
                .packet_payload
                .saturating_mul(self.expected_packets.saturating_sub(1))
                + last_len;
            let mut buf = self.buffer;
            if buf.len() > used {
                buf.truncate(used);
            }
            return Some(buf.freeze());
        }

        // Otherwise, compact the data to remove any gaps introduced by
        // shorter packets occurring before the last packet.
        let total: usize = self.lengths.iter().sum();
        let mut out = BytesMut::with_capacity(total);
        for (i, &len) in self.lengths.iter().enumerate() {
            if len == 0 {
                continue;
            }
            let start = i * self.packet_payload;
            let end = start + len;
            out.extend_from_slice(&self.buffer[start..end]);
        }
        Some(out.freeze())
    }
}

/// Helper struct tracking resend attempts for a given block.
#[derive(Debug, Clone)]
pub struct ResendPlanner {
    retries: u32,
    max_retries: u32,
    base_delay: Duration,
    next_deadline: Instant,
}

impl ResendPlanner {
    pub fn new(max_retries: u32, base_delay: Duration) -> Self {
        Self {
            retries: 0,
            max_retries,
            base_delay,
            next_deadline: Instant::now(),
        }
    }

    /// Determine whether a resend can be attempted at the provided instant.
    pub fn should_resend(&self, now: Instant) -> bool {
        self.retries < self.max_retries && now >= self.next_deadline
    }

    /// Record a resend attempt and compute the next deadline.
    pub fn record_attempt(&mut self, now: Instant, jitter: Duration) {
        self.retries += 1;
        let base = self
            .base_delay
            .checked_mul(self.retries)
            .unwrap_or(self.base_delay);
        self.next_deadline = now + base + jitter;
    }

    /// Whether the resend planner exhausted all retries.
    pub fn is_exhausted(&self) -> bool {
        self.retries >= self.max_retries
    }
}

/// Representation of a fully reassembled frame ready for consumption.
#[derive(Debug, Clone)]
pub struct CompletedFrame {
    pub block_id: u64,
    pub timestamp: Instant,
    pub payload: Bytes,
}

/// Frame queue used for communicating between the receiver task and the
/// application.
#[derive(Debug)]
pub struct FrameQueue {
    inner: VecDeque<CompletedFrame>,
    capacity: usize,
}

impl FrameQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, frame: CompletedFrame, stats: &StreamStatsAccumulator) {
        if self.inner.len() == self.capacity {
            self.inner.pop_front();
            stats.record_backpressure_drop();
        }
        self.inner.push_back(frame);
    }

    pub fn pop(&mut self) -> Option<CompletedFrame> {
        self.inner.pop_front()
    }
}

/// Coalesce missing packet ranges into resend requests.
pub fn coalesce_missing(bitmap: &PacketBitmap, max_range: usize) -> Vec<RangeInclusive<u32>> {
    bitmap
        .missing_ranges()
        .into_iter()
        .flat_map(|range| split_range(range, max_range))
        .collect()
}

fn split_range(range: RangeInclusive<u32>, max_len: usize) -> Vec<RangeInclusive<u32>> {
    let start = *range.start() as usize;
    let end = *range.end() as usize;
    if max_len == 0 {
        return vec![range];
    }
    let mut result = Vec::new();
    let mut current = start;
    while current <= end {
        let upper = (current + max_len - 1).min(end);
        result.push(current as u32..=upper as u32);
        current = upper + 1;
    }
    result
}

/// Zero-copy block assembly state machine.
#[derive(Debug)]
pub struct Reassembler {
    active: Option<FrameAssembly>,
    packet_payload: usize,
    stats: StreamStatsAccumulator,
}

impl Reassembler {
    pub fn new(packet_payload: usize, stats: StreamStatsAccumulator) -> Self {
        Self {
            active: None,
            packet_payload,
            stats,
        }
    }

    /// Start a new block, evicting the previous one when necessary.
    pub fn start_block(&mut self, block_id: u64, expected_packets: usize, buffer: BytesMut) {
        let deadline = Instant::now() + Duration::from_millis(50);
        self.active = Some(FrameAssembly::new(
            block_id,
            expected_packets,
            self.packet_payload,
            buffer,
            deadline,
        ));
    }

    /// Insert a packet belonging to the active block.
    pub fn push_packet(&mut self, packet_id: usize, payload: &[u8]) {
        if let Some(assembly) = self.active.as_mut()
            && assembly.ingest(packet_id, payload)
        {
            self.stats.record_packet();
        }
    }

    /// Attempt to finish the current block.
    pub fn finish_block(&mut self) -> Option<Bytes> {
        self.active.take().and_then(FrameAssembly::finish)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_multiple_chunks() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&0x0001u16.to_be_bytes());
        payload.extend_from_slice(&0u16.to_be_bytes());
        payload.extend_from_slice(&4u32.to_be_bytes());
        payload.extend_from_slice(&[1, 2, 3, 4]);
        payload.extend_from_slice(&0x0002u16.to_be_bytes());
        payload.extend_from_slice(&0u16.to_be_bytes());
        payload.extend_from_slice(&2u32.to_be_bytes());
        payload.extend_from_slice(&[5, 6]);
        let chunks = parse_chunks(&payload);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].id, 0x0001);
        assert_eq!(chunks[0].data.as_ref(), &[1, 2, 3, 4]);
        assert_eq!(chunks[1].id, 0x0002);
        assert_eq!(chunks[1].data.as_ref(), &[5, 6]);
    }

    #[test]
    fn truncated_chunk_is_ignored() {
        let payload = vec![0u8; 6];
        let chunks = parse_chunks(&payload);
        assert!(chunks.is_empty());
    }

    #[test]
    fn parse_chunks_tolerates_padding() {
        for _ in 0..128 {
            let count = fastrand::usize(..6);
            let mut payload = Vec::new();
            let mut entries = Vec::new();
            for _ in 0..count {
                let id = fastrand::u16(..);
                let len = fastrand::usize(..16);
                let mut data = vec![0u8; len];
                for byte in &mut data {
                    *byte = fastrand::u8(..);
                }
                payload.extend_from_slice(&id.to_be_bytes());
                payload.extend_from_slice(&0u16.to_be_bytes());
                payload.extend_from_slice(&(data.len() as u32).to_be_bytes());
                payload.extend_from_slice(&data);
                entries.push((id, data));
            }
            let padding_len = fastrand::usize(..8);
            for _ in 0..padding_len {
                payload.push(fastrand::u8(..));
            }
            let parsed = parse_chunks(&payload);
            assert!(parsed.len() <= entries.len());
            for (idx, chunk) in parsed.iter().enumerate() {
                assert_eq!(chunk.id, entries[idx].0);
                assert_eq!(chunk.data.as_ref(), entries[idx].1.as_slice());
            }
        }
    }

    #[test]
    fn bitmap_missing_ranges_coalesce() {
        let mut bitmap = PacketBitmap::new(10);
        for &idx in &[0usize, 1, 5, 6, 9] {
            bitmap.set(idx);
        }
        let ranges = bitmap.missing_ranges();
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0], 2..=4);
        assert_eq!(ranges[1], 7..=8);
    }

    #[test]
    fn coalesce_splits_large_ranges() {
        let mut bitmap = PacketBitmap::new(20);
        for idx in [0usize, 1, 2, 18, 19] {
            bitmap.set(idx);
        }
        let ranges = coalesce_missing(&bitmap, 4);
        assert_eq!(ranges, vec![3..=6, 7..=10, 11..=14, 15..=17]);
    }

    #[test]
    fn reassembler_finishes_frame() {
        let stats = StreamStatsAccumulator::new();
        let mut reassembler = Reassembler::new(4, stats);
        reassembler.start_block(1, 3, BytesMut::with_capacity(12));
        reassembler.push_packet(0, &[1, 2, 3]);
        reassembler.push_packet(1, &[4, 5, 6]);
        reassembler.push_packet(2, &[7, 8, 9]);
        let frame = reassembler.finish_block().expect("frame");
        assert_eq!(frame.as_ref(), &[1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }
}
