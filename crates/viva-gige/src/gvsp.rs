//! GVSP packet parsing and stream configuration.
//!
//! The GigE Vision Streaming Protocol delivers a block (one image) as a leader,
//! data packets and a trailer over UDP. [`parse_packet_ref`] reads one datagram
//! without copying it; the receive engine in `viva-genicam` assembles blocks
//! from what it returns.

use std::net::Ipv4Addr;
use std::ops::RangeInclusive;

use crate::nic::Iface;
use bytes::Buf;
use thiserror::Error;
use tracing::debug;

/// GVSP payload type for image data. Bits 13..0 of the 16-bit field carry the
/// type; bit 14 ([`PAYLOAD_TYPE_EXTENDED_CHUNK`]) flags extended chunk data,
/// so `0x4001` is an image with chunks (TC-05). aravis masks the same way
/// (`payload_type & 0x3fff`, `& 0x4000` for chunks in `arvgvspprivate.h`).
pub const PAYLOAD_TYPE_IMAGE: u16 = 0x0001;

/// Bit 14 of the payload type: the block carries extended chunk data.
pub const PAYLOAD_TYPE_EXTENDED_CHUNK: u16 = 0x4000;

/// GVSP payload type for chunk data: a block made only of chunks, whose
/// leader carries no image geometry (`reserved | payload type | timestamp`)
/// and whose trailer reports the payload data length where an image's
/// reports Size Y. Layout corroborated by Wireshark's
/// `dissect_chunk_data_leader` / `_trailer`.
pub const PAYLOAD_TYPE_CHUNK_DATA: u16 = 0x0004;

/// GVSP payload type for GigE Vision 1.x's extended chunk data, deprecated
/// by GigE Vision 2.0 for an image with the chunk flag (`0x4001`), whose
/// layout it shares: an image leader, a payload of chunks with the image
/// first, and a trailer of reserved | payload type | payload data length |
/// Size Y | chunk layout ID. An Allied Vision Manta G-125B (GigE Vision 1.2)
/// sends it with `ChunkModeActive` on. Layout corroborated by Wireshark's
/// `dissect_extended_chunk_data_leader` / `_trailer`; aravis treats it as an
/// image with chunks.
pub const PAYLOAD_TYPE_EXTENDED_CHUNK_DATA: u16 = 0x0005;

/// Bits of the payload type that name the type itself.
const PAYLOAD_TYPE_MASK: u16 = 0x3FFF;

/// Whether `payload_type` is an image, with or without extended chunk data:
/// GigE Vision 1.x's [`PAYLOAD_TYPE_EXTENDED_CHUNK_DATA`] is one too.
pub fn is_image_payload(payload_type: u16) -> bool {
    matches!(
        payload_type & PAYLOAD_TYPE_MASK,
        PAYLOAD_TYPE_IMAGE | PAYLOAD_TYPE_EXTENDED_CHUNK_DATA
    )
}

/// Whether `payload_type` is chunk data, with or without the chunk flag.
pub fn is_chunk_data_payload(payload_type: u16) -> bool {
    payload_type & PAYLOAD_TYPE_MASK == PAYLOAD_TYPE_CHUNK_DATA
}

/// Whether `payload_type` carries extended chunk data: it has the chunk flag
/// (bit 14), or is GigE Vision 1.x's [`PAYLOAD_TYPE_EXTENDED_CHUNK_DATA`].
pub fn has_extended_chunks(payload_type: u16) -> bool {
    payload_type & PAYLOAD_TYPE_EXTENDED_CHUNK != 0
        || payload_type & PAYLOAD_TYPE_MASK == PAYLOAD_TYPE_EXTENDED_CHUNK_DATA
}

/// Size of the standard GVSP header.
pub const GVSP_HEADER_SIZE: usize = 8;

/// Size of the extended-ID GVSP header (GigE Vision 2.0).
pub const GVSP_EXTENDED_HEADER_SIZE: usize = 20;

/// Extended ID flag: bit 7 of the packet-format byte.
const EXTENDED_ID_FLAG: u8 = 0x80;

/// Packet format: data leader.
const FORMAT_LEADER: u8 = 0x01;
/// Packet format: data trailer.
const FORMAT_TRAILER: u8 = 0x02;
/// Packet format: generic data payload.
const FORMAT_PAYLOAD: u8 = 0x03;

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
    /// GVSP packet size in force.
    pub packet_size: Option<u32>,
    /// `GevSCPD` written, in timestamp ticks; `None` when left alone.
    pub packet_delay: Option<u32>,
    /// The only address GVSP is accepted from: the camera's. Datagrams from
    /// any other source are ignored and counted.
    pub source_filter: Option<Ipv4Addr>,
}

/// Errors raised while handling GVSP packets.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum GvspError {
    #[error("unsupported packet type: {0}")]
    /// A packet of a kind the parser does not handle.
    Unsupported(&'static str),
    #[error("invalid packet: {0}")]
    /// A packet that does not have the shape GVSP gives it.
    Invalid(&'static str),
}

/// Length of a chunk tag: chunk ID and data length, 32 bits each.
const CHUNK_TAG_LEN: usize = 8;

/// One chunk of a GVSP chunk payload, borrowing the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkRaw<'a> {
    /// Chunk ID: 32 bits, as a `<Port>`'s `<ChunkID>` names it.
    pub id: u32,
    /// Where the chunk's data starts in the payload.
    pub offset: usize,
    /// The chunk's data, without its tag.
    pub data: &'a [u8],
}

/// A chunk payload that does not tile into tagged chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ChunkError {
    /// What remains at the front of the payload is too short for a tag.
    #[error(
        "{remaining} bytes remain at offset 0 of the chunk data, too few for an \
         8-byte chunk tag"
    )]
    TruncatedTag {
        /// Bytes left before the last chunk parsed.
        remaining: usize,
    },
    /// A tag claims more data than precedes it.
    #[error(
        "chunk {id:#010x} tagged at offset {tag_offset} claims {length} bytes of data, \
         more than the {tag_offset} before it"
    )]
    LengthPastStart {
        /// The chunk ID in the tag.
        id: u32,
        /// Where the tag starts.
        tag_offset: usize,
        /// The data length it claims.
        length: u32,
    },
}

/// Split a GigE Vision chunk payload into its chunks, in payload order.
///
/// Each chunk is its data followed by an 8-byte tag — the chunk ID, then the
/// data length, both 32-bit big-endian — so the payload can only be read from
/// its end: the last 8 bytes are the last chunk's tag, its length says where
/// that chunk's data begins, and the 8 bytes before that are the previous
/// tag. With payload type `0x4001` the image is the first chunk, and the scan
/// ends exactly at offset 0 after it.
///
/// The GigE Vision specification is not available to us; the layout is
/// corroborated by aravis (`arv_buffer_get_chunk_data` walks
/// `{u32 id; u32 size}` tags back from the received size, big-endian for
/// GigE Vision) and by the vendor corpus's 32-bit `<ChunkID>`s such as
/// FLIR's `0x0508000E` (ADR-0026). `payload` must end where the camera's data
/// ends — the received length, not a buffer sized from `PayloadSize`.
///
/// Remote input: a tag that claims more than precedes it, or a front too
/// short for a tag, is an error naming the offset. A zero-length chunk is a
/// chunk; the scan still moves back by its tag, so it cannot loop.
pub fn parse_chunks(payload: &[u8]) -> Result<Vec<ChunkRaw<'_>>, ChunkError> {
    let mut chunks = Vec::new();
    let mut end = payload.len();
    while end > 0 {
        let tag_offset = end
            .checked_sub(CHUNK_TAG_LEN)
            .ok_or(ChunkError::TruncatedTag { remaining: end })?;
        let tag = &payload[tag_offset..end];
        let id = u32::from_be_bytes([tag[0], tag[1], tag[2], tag[3]]);
        let length = u32::from_be_bytes([tag[4], tag[5], tag[6], tag[7]]);
        let start = tag_offset
            .checked_sub(length as usize)
            .ok_or(ChunkError::LengthPastStart {
                id,
                tag_offset,
                length,
            })?;
        chunks.push(ChunkRaw {
            id,
            offset: start,
            data: &payload[start..tag_offset],
        });
        end = start;
    }
    chunks.reverse();
    Ok(chunks)
}

/// The header every GVSP packet starts with, in either layout.
///
/// | Bytes | Standard (8)        | Extended ids (20)           |
/// |-------|---------------------|-----------------------------|
/// | 0–1   | status              | status                      |
/// | 2–3   | block id (16 bits)  | flags                       |
/// | 4     | packet format       | `0x80` \| packet format     |
/// | 5–7   | packet id (24 bits) | unused                      |
/// | 8–15  |                     | block id (64 bits)          |
/// | 16–19 |                     | packet id (32 bits)         |
///
/// The extended layout is corroborated by aravis's `ArvGvspExtendedHeader` and
/// by a LUCID TRT009S-E capture, whose bytes 2–3 and 5–7 are zero (the
/// `lucid_evt30_*` tests below).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GvspHeader {
    /// Status word, raw: decode it with [`GigeStatus`](crate::GigeStatus).
    /// Bit 15 marks an error (aravis `arv_gvsp_packet_status_is_error`);
    /// `0x0100` flags a resent packet.
    pub status: u16,
    /// Block id: 16 bits widened, or the 64-bit extended id.
    pub block_id: u64,
    /// Packet id: 24 bits widened, or the 32-bit extended id.
    pub packet_id: u32,
    /// Whether the packet carries the 20-byte extended-ID header.
    pub extended_ids: bool,
    /// Bytes of header before the packet's own data: 8 or 20.
    pub header_len: usize,
}

/// Image fields of a leader whose payload type is an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageLeader {
    /// PFNC pixel format code.
    pub pixel_format: u32,
    /// Size X in pixels.
    pub width: u32,
    /// Size Y in lines.
    pub height: u32,
    /// Offset X.
    pub offset_x: u32,
    /// Offset Y.
    pub offset_y: u32,
    /// Bytes of padding at the end of each line.
    pub padding_x: u16,
    /// Bytes of padding at the end of the image.
    pub padding_y: u16,
}

/// The fields of a data leader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GvspLeader {
    /// The full 16-bit payload type, chunk flag included (TC-05).
    pub payload_type: u16,
    /// Device timestamp.
    pub timestamp: u64,
    /// Image fields; `None` for a payload type that is not an image, whose
    /// leader has a different layout.
    pub image: Option<ImageLeader>,
}

/// The fields of a data trailer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GvspTrailer {
    /// Payload type the trailer closes, chunk flag included.
    pub payload_type: u16,
    /// Size Y for an image: the lines the block delivered. For a chunk-data
    /// block (`0x0004`) the same field is the payload data length.
    pub size_y: u32,
    /// With the chunk flag (`0x4000`), the chunk data payload length the
    /// trailer reports after its 8 bytes; `None` without one.
    pub chunk_payload_length: Option<u32>,
    /// With the chunk flag, the chunk layout ID: the device changes it when
    /// the set or position of chunks changes, so a receiver may cache a
    /// layout while it holds.
    pub chunk_layout_id: Option<u32>,
}

/// What a GVSP packet carries after its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GvspContent<'a> {
    /// Data leader (packet id 0).
    Leader(GvspLeader),
    /// Generic data payload, borrowed from the datagram.
    Payload(&'a [u8]),
    /// Data trailer.
    Trailer(GvspTrailer),
    /// A packet format this crate does not assemble: all-in (`0x04`),
    /// H.264 (`0x05`), multi-zone (`0x06`), multipart (`0x07`), GenDC (`0x08`)
    /// or anything else. Kept as a value so a receiver can count it rather than
    /// lose it as a parse error.
    Unsupported {
        /// The packet-format nibble.
        packet_format: u8,
    },
}

/// A GVSP packet borrowing the datagram it was parsed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GvspPacketRef<'a> {
    /// The header.
    pub header: GvspHeader,
    /// What follows it.
    pub content: GvspContent<'a>,
}

/// Parse one GVSP datagram without copying it.
///
/// Fails only on a datagram too short for the header its own flag announces,
/// or for the fixed fields of a leader or trailer.
pub fn parse_packet_ref(datagram: &[u8]) -> Result<GvspPacketRef<'_>, GvspError> {
    let header = parse_header(datagram)?;
    let packet_format = datagram[4] & 0x0F;
    let body = &datagram[header.header_len..];
    let content = match packet_format {
        FORMAT_LEADER => GvspContent::Leader(parse_leader(body)?),
        FORMAT_PAYLOAD => GvspContent::Payload(body),
        FORMAT_TRAILER => GvspContent::Trailer(parse_trailer(header.block_id, body)?),
        other => GvspContent::Unsupported {
            packet_format: other,
        },
    };
    Ok(GvspPacketRef { header, content })
}

/// Parse only the header of a GVSP datagram.
///
/// What a packet that carries nothing else needs: a device answering a resend
/// request it cannot serve sends a bare header with an error status, which
/// [`parse_packet_ref`] would refuse as a truncated leader or trailer.
pub fn parse_header(datagram: &[u8]) -> Result<GvspHeader, GvspError> {
    if datagram.len() < GVSP_HEADER_SIZE {
        return Err(GvspError::Invalid("GVSP header truncated"));
    }
    let status = u16::from_be_bytes([datagram[0], datagram[1]]);
    let format_byte = datagram[4];
    let extended_ids = format_byte & EXTENDED_ID_FLAG != 0;

    let header = if extended_ids {
        if datagram.len() < GVSP_EXTENDED_HEADER_SIZE {
            return Err(GvspError::Invalid("extended GVSP header truncated"));
        }
        let mut block = [0u8; 8];
        block.copy_from_slice(&datagram[8..16]);
        GvspHeader {
            status,
            block_id: u64::from_be_bytes(block),
            packet_id: u32::from_be_bytes([datagram[16], datagram[17], datagram[18], datagram[19]]),
            extended_ids,
            header_len: GVSP_EXTENDED_HEADER_SIZE,
        }
    } else {
        GvspHeader {
            status,
            block_id: u64::from(u16::from_be_bytes([datagram[2], datagram[3]])),
            packet_id: u32::from_be_bytes([0, datagram[5], datagram[6], datagram[7]]),
            extended_ids,
            header_len: GVSP_HEADER_SIZE,
        }
    };
    Ok(header)
}

/// Parse a data leader's payload.
///
/// | Offset | Size | Field        |
/// |--------|------|--------------|
/// |      0 |    2 | Reserved     |
/// |      2 |    2 | Payload type |
/// |      4 |    8 | Timestamp    |
/// |     12 |    4 | Pixel format |
/// |     16 |    4 | Size X       |
/// |     20 |    4 | Size Y       |
/// |     24 |    4 | Offset X     |
/// |     28 |    4 | Offset Y     |
/// |     32 |    2 | Padding X    |
/// |     34 |    2 | Padding Y    |
///
/// Offsets and padding are optional: a leader that stops after Size Y still
/// describes its image, so it is read with zeros there rather than refused.
fn parse_leader(body: &[u8]) -> Result<GvspLeader, GvspError> {
    if body.len() < 12 {
        return Err(GvspError::Invalid("leader payload truncated"));
    }
    let mut cursor = body;
    let _reserved = cursor.get_u16();
    let payload_type = cursor.get_u16();
    let timestamp = cursor.get_u64();
    let image = if is_image_payload(payload_type) {
        if cursor.len() < 12 {
            return Err(GvspError::Invalid("image leader truncated"));
        }
        let pixel_format = cursor.get_u32();
        let width = cursor.get_u32();
        let height = cursor.get_u32();
        let (offset_x, offset_y) = if cursor.len() >= 8 {
            (cursor.get_u32(), cursor.get_u32())
        } else {
            (0, 0)
        };
        let (padding_x, padding_y) = if cursor.len() >= 4 {
            (cursor.get_u16(), cursor.get_u16())
        } else {
            (0, 0)
        };
        Some(ImageLeader {
            pixel_format,
            width,
            height,
            offset_x,
            offset_y,
            padding_x,
            padding_y,
        })
    } else {
        None
    };
    Ok(GvspLeader {
        payload_type,
        timestamp,
        image,
    })
}

/// Parse a data trailer's payload.
///
/// | Offset | Size | Field                                         |
/// |--------|------|-----------------------------------------------|
/// |      0 |    2 | Reserved                                      |
/// |      2 |    2 | Payload type                                  |
/// |      4 |    4 | Size Y (image), payload data length (`0x0004`)|
/// |      8 |    4 | Chunk data payload length, with `0x4000`      |
/// |     12 |    4 | Chunk layout ID, with `0x4000`                |
///
/// GigE Vision 1.x's extended chunk data (`0x0005`) puts the payload data
/// length at 4, Size Y at 8 and the chunk layout ID at 12.
///
/// Chunk data travels in the data packets, not here: with the chunk flag set
/// the trailer only adds the two fields at 8 and 12. Those 8 bytes used to be
/// handed to the chunk parser as a "chunk region" (TC-06). The layout is
/// corroborated by Wireshark's `dissect_packet_trailer` and
/// `dissect_extra_chunk_info`; the specification is not available to us.
///
/// `status` comes from the packet header, not from here; the first two bytes
/// of this payload are reserved and carry no status.
fn parse_trailer(block_id: u64, body: &[u8]) -> Result<GvspTrailer, GvspError> {
    if body.len() < 2 {
        return Err(GvspError::Invalid("trailer truncated"));
    }
    // The specification fixes this payload at 8 bytes. A device that sends
    // fewer still gets its frame delivered, with the fields it left out
    // reported as zero.
    if body.len() < 8 {
        debug!(
            block_id,
            len = body.len(),
            "trailer payload shorter than the specified 8 bytes"
        );
        return Ok(GvspTrailer {
            payload_type: 0,
            size_y: 0,
            chunk_payload_length: None,
            chunk_layout_id: None,
        });
    }
    let mut cursor = &body[2..];
    let payload_type = cursor.get_u16();
    if payload_type & PAYLOAD_TYPE_MASK == PAYLOAD_TYPE_EXTENDED_CHUNK_DATA && cursor.len() >= 12 {
        // GigE Vision 1.x: the payload data length comes first, then Size Y.
        let length = cursor.get_u32();
        let size_y = cursor.get_u32();
        return Ok(GvspTrailer {
            payload_type,
            size_y,
            chunk_payload_length: Some(length),
            chunk_layout_id: Some(cursor.get_u32()),
        });
    }
    let size_y = cursor.get_u32();
    let (chunk_payload_length, chunk_layout_id) =
        if has_extended_chunks(payload_type) && cursor.len() >= 8 {
            (Some(cursor.get_u32()), Some(cursor.get_u32()))
        } else {
            (None, None)
        };
    Ok(GvspTrailer {
        payload_type,
        size_y,
        chunk_payload_length,
        chunk_layout_id,
    })
}

/// Bitmap tracking received packets within a block.
///
/// Reusable: [`PacketBitmap::reset`] clears it for another block while keeping
/// its allocation.
#[derive(Debug, Clone, Default)]
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

    /// Clear every bit and track `total` packets, reusing the allocation.
    pub fn reset(&mut self, total: usize) {
        self.words.clear();
        self.words.resize(total.div_ceil(64), 0);
        self.received = 0;
        self.total = total;
    }

    /// Grow to track at least `total` packets, keeping the bits already set.
    pub fn grow(&mut self, total: usize) {
        if total > self.total {
            self.words.resize(total.div_ceil(64), 0);
            self.total = total;
        }
    }

    /// Packets this bitmap tracks.
    pub fn len(&self) -> usize {
        self.total
    }

    /// Whether the bitmap tracks no packets.
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// Packets marked received.
    pub fn received(&self) -> usize {
        self.received
    }

    fn mask_for(&self, packet_id: usize) -> (usize, u64) {
        let word = packet_id / 64;
        let bit = packet_id % 64;
        (word, 1u64 << bit)
    }

    /// Whether a packet index is marked received.
    pub fn contains(&self, packet_id: usize) -> bool {
        if packet_id >= self.total {
            return false;
        }
        let (word, mask) = self.mask_for(packet_id);
        self.words[word] & mask != 0
    }

    /// Mark a packet index as received. Returns `false` for an index already
    /// marked or beyond the bitmap.
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

    /// Unmark a packet index. Returns `false` for an index not marked.
    pub fn clear(&mut self, packet_id: usize) -> bool {
        if !self.contains(packet_id) {
            return false;
        }
        let (word, mask) = self.mask_for(packet_id);
        self.words[word] &= !mask;
        self.received -= 1;
        true
    }

    /// Whether the first `count` indices are all marked received.
    pub fn first_n_complete(&self, count: usize) -> bool {
        if count > self.total {
            return false;
        }
        let full = count / 64;
        if self.words[..full].iter().any(|w| *w != u64::MAX) {
            return false;
        }
        let rest = count % 64;
        rest == 0 || self.words[full] & ((1u64 << rest) - 1) == (1u64 << rest) - 1
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Extended-ID leader captured from a LUCID TRT009S-E producing EVT 3.0.
    /// Keeping this literal makes the event-stream path rest on hardware bytes,
    /// not on a packet generated from our own interpretation.
    #[test]
    fn lucid_evt30_capture_uses_an_image_leader() {
        #[rustfmt::skip]
        const LEADER: [u8; 56] = [
            0x00, 0x00,             // status
            0x00, 0x00,             // 16-bit block-id compatibility field
            0x81,                   // extended-ID leader
            0x00, 0x00, 0x00,       // 24-bit packet-id compatibility field
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, // block id
            0x00, 0x00, 0x00, 0x00, // packet id
            0x00, 0x00,             // reserved
            0x00, 0x01,             // IMAGE payload type
            0x00, 0x00, 0x42, 0x2d, 0x66, 0xd4, 0x39, 0xc8, // timestamp
            0x81, 0x10, 0x0e, 0x30, // EVT 3.0 data format
            0x00, 0x00, 0x00, 0x01, // size X
            0x00, 0x00, 0xfa, 0x00, // size Y (64000)
            0x00, 0x00, 0x00, 0x00, // offset X
            0x00, 0x00, 0x00, 0x00, // offset Y
            0x00, 0x00, 0x00, 0x00, // padding X/Y
        ];

        let packet = parse_packet_ref(&LEADER).expect("parse captured LUCID leader");
        let GvspContent::Leader(leader) = packet.content else {
            panic!("expected leader");
        };
        let image = leader.image.expect("an IMAGE leader has image fields");

        assert_eq!(packet.header.block_id, 1);
        assert_eq!(packet.header.packet_id, 0);
        assert_eq!(packet.header.header_len, GVSP_EXTENDED_HEADER_SIZE);
        assert_eq!(leader.payload_type, PAYLOAD_TYPE_IMAGE);
        assert_eq!(leader.timestamp, 0x0000_422d_66d4_39c8);
        assert_eq!(image.pixel_format, 0x8110_0e30);
        assert_eq!((image.width, image.height), (1, 64_000));
    }

    /// Trailer paired with [`lucid_evt30_capture_uses_an_image_leader`].
    #[test]
    fn lucid_evt30_capture_trailer_reports_payload_bytes() {
        #[rustfmt::skip]
        const TRAILER: [u8; 28] = [
            0x00, 0x00,             // status
            0x00, 0x00,             // 16-bit block-id compatibility field
            0x82,                   // extended-ID trailer
            0x00, 0x00, 0x00,       // 24-bit packet-id compatibility field
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, // block id
            0x00, 0x00, 0x00, 0x02, // packet id
            0x00, 0x00,             // reserved
            0x00, 0x01,             // IMAGE payload type
            0x00, 0x00, 0x03, 0x38, // captured payload length (824)
        ];

        let packet = parse_packet_ref(&TRAILER).expect("parse captured LUCID trailer");
        let GvspContent::Trailer(trailer) = packet.content else {
            panic!("expected trailer");
        };

        assert_eq!(packet.header.block_id, 1);
        assert_eq!(packet.header.packet_id, 2);
        assert_eq!(packet.header.status, 0);
        assert_eq!(trailer.payload_type, PAYLOAD_TYPE_IMAGE);
        assert_eq!(trailer.size_y, 824);
        assert_eq!(trailer.chunk_payload_length, None);
    }

    /// The leader and trailer of one block from an Allied Vision Manta G-125B
    /// (GigE Vision 1.2, firmware 00.01.44.28514) with `ChunkModeActive` on,
    /// received on a socket of our own: payload type `0x0005`, GigE Vision
    /// 1.x's extended chunk data. The leader is an image's; the trailer is
    /// payload data length (1 245 544: the image, its tag, 40 bytes of the
    /// camera's chunk and its tag), Size Y, chunk layout ID — the layout
    /// Wireshark's `dissect_extended_chunk_data_trailer` reads.
    #[test]
    fn a_gige_vision_1_extended_chunk_block_is_an_image_with_chunks() {
        #[rustfmt::skip]
        const LEADER: [u8; 44] = [
            0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00,
            0x00, 0x01,             // reserved
            0x00, 0x05,             // EXTENDED CHUNK DATA
            0x00, 0x00, 0x00, 0x2e, 0xf1, 0xc1, 0x84, 0xe8, // timestamp
            0x01, 0x08, 0x00, 0x01, // Mono8
            0x00, 0x00, 0x05, 0x0c, // 1292
            0x00, 0x00, 0x03, 0xc4, // 964
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ];
        #[rustfmt::skip]
        const TRAILER: [u8; 24] = [
            0x00, 0x00, 0x00, 0x01, 0x02, 0x00, 0x03, 0x54,
            0x00, 0x00,             // reserved
            0x00, 0x05,             // EXTENDED CHUNK DATA
            0x00, 0x13, 0x01, 0x68, // payload data length
            0x00, 0x00, 0x03, 0xc4, // size Y
            0x00, 0x00, 0x00, 0x01, // chunk layout ID
        ];
        let GvspContent::Leader(leader) = parse_packet_ref(&LEADER).expect("leader").content else {
            panic!("expected a leader");
        };
        let image = leader.image.expect("an image leader");
        assert_eq!(
            (image.pixel_format, image.width, image.height),
            (0x0108_0001, 1292, 964)
        );
        assert!(is_image_payload(leader.payload_type));
        assert!(has_extended_chunks(leader.payload_type));

        let GvspContent::Trailer(trailer) = parse_packet_ref(&TRAILER).expect("trailer").content
        else {
            panic!("expected a trailer");
        };
        assert_eq!(trailer.payload_type, PAYLOAD_TYPE_EXTENDED_CHUNK_DATA);
        assert_eq!(trailer.size_y, 964);
        assert_eq!(trailer.chunk_payload_length, Some(1_245_544));
        assert_eq!(trailer.chunk_layout_id, Some(1));
    }

    /// A standard-header image leader with `payload_type`, written from the
    /// field table (header 8, then reserved 2 | payload type 2 | timestamp 8 |
    /// pixel format 4 | size x 4 | size y 4 | offsets 8 | padding 4).
    fn golden_leader(payload_type: u16) -> Vec<u8> {
        let mut pkt = vec![0x00, 0x00, 0x00, 0x07, 0x01, 0x00, 0x00, 0x00];
        pkt.extend_from_slice(&0u16.to_be_bytes());
        pkt.extend_from_slice(&payload_type.to_be_bytes());
        pkt.extend_from_slice(&0x0102_0304_0506_0708u64.to_be_bytes());
        pkt.extend_from_slice(&0x0108_0001u32.to_be_bytes());
        pkt.extend_from_slice(&640u32.to_be_bytes());
        pkt.extend_from_slice(&480u32.to_be_bytes());
        pkt.extend_from_slice(&[0u8; 12]);
        pkt
    }

    /// TC-05: the payload type is 16 bits, and bit 14 flags extended chunk
    /// data. Truncated to a byte, 0x4001 reads as 0x01 and the flag is gone.
    #[test]
    fn leader_keeps_the_extended_chunk_bit() {
        let pkt = golden_leader(0x4001);
        let GvspContent::Leader(leader) = parse_packet_ref(&pkt).expect("parse leader").content
        else {
            panic!("expected a leader");
        };
        assert_eq!(leader.payload_type, 0x4001);
    }

    /// A GVSP data trailer, assembled from the specification rather than from
    /// what our own parser or fake camera happen to produce (ADR-0019).
    ///
    /// Standard header (8 bytes): `status(2) | block_id(2) | format(1) |
    /// packet_id(3)`, then the trailer payload (8 bytes): `reserved(2) |
    /// payload_type(2) | size_y(4)`, then whatever `extra` holds.
    fn golden_trailer(status: u16, payload_type: u16, size_y: u32, extra: &[u8]) -> Vec<u8> {
        let mut pkt = Vec::new();
        pkt.extend_from_slice(&status.to_be_bytes());
        pkt.extend_from_slice(&0x0007u16.to_be_bytes()); // block_id
        pkt.push(0x02); // packet format: trailer
        pkt.extend_from_slice(&[0x00, 0x00, 0x42]); // packet_id (24-bit)
        pkt.extend_from_slice(&0u16.to_be_bytes()); // reserved
        pkt.extend_from_slice(&payload_type.to_be_bytes());
        pkt.extend_from_slice(&size_y.to_be_bytes());
        pkt.extend_from_slice(extra);
        pkt
    }

    /// The regression behind the per-frame `chunk header truncated
    /// remaining=6` a JAI produced on issue #70: a trailer closing a plain
    /// image block is 8 bytes — `payload_type` and `size_y` were what the old
    /// parser reported as a truncated chunk header.
    #[test]
    fn a_plain_image_trailer_has_no_chunk_fields() {
        let pkt = golden_trailer(0, PAYLOAD_TYPE_IMAGE, 1536, &[]);
        assert_eq!(pkt.len(), 16, "8-byte header + 8-byte trailer payload");

        let packet = parse_packet_ref(&pkt).expect("parse trailer");
        let GvspContent::Trailer(trailer) = packet.content else {
            panic!("expected a trailer");
        };

        assert_eq!(packet.header.block_id, 0x0007);
        assert_eq!(packet.header.packet_id, 0x42);
        assert_eq!(packet.header.status, 0);
        assert_eq!(trailer.payload_type, 0x0001);
        assert_eq!(trailer.size_y, 1536);
        assert_eq!(trailer.chunk_payload_length, None);
        assert_eq!(trailer.chunk_layout_id, None);
    }

    /// TC-06: with the chunk flag the trailer grows by two 32-bit fields —
    /// the chunk data payload length, then the chunk layout ID (Wireshark's
    /// `dissect_extra_chunk_info`, corroboration only). The chunks themselves
    /// travel in the data packets; these eight bytes used to be parsed as a
    /// chunk region.
    #[test]
    fn an_extended_chunk_trailer_reports_length_and_layout_id() {
        let mut extra = Vec::new();
        extra.extend_from_slice(&0x0004_B044u32.to_be_bytes()); // chunk payload length
        extra.extend_from_slice(&0xC0DE_0001u32.to_be_bytes()); // chunk layout id
        let pkt = golden_trailer(0, 0x4001, 480, &extra);
        let GvspContent::Trailer(trailer) = parse_packet_ref(&pkt).expect("parse trailer").content
        else {
            panic!("expected a trailer");
        };
        assert_eq!(trailer.payload_type, 0x4001);
        assert_eq!(trailer.size_y, 480);
        assert_eq!(trailer.chunk_payload_length, Some(0x0004_B044));
        assert_eq!(trailer.chunk_layout_id, Some(0xC0DE_0001));
    }

    /// The status word lives in the packet header, not in the trailer payload
    /// whose first two bytes are reserved. Reading it from the payload meant
    /// the receiver's `status != 0` frame check tested the wrong field.
    #[test]
    fn trailer_status_comes_from_the_packet_header() {
        // A non-zero header status with an all-zero (reserved) payload prefix:
        // the old code read the payload and saw success.
        let pkt = golden_trailer(0x8004, PAYLOAD_TYPE_IMAGE, 0, &[]);
        let packet = parse_packet_ref(&pkt).expect("parse trailer");
        assert!(matches!(packet.content, GvspContent::Trailer(_)));
        assert_eq!(packet.header.status, 0x8004);
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
    fn bitmap_reset_and_grow_keep_their_promises() {
        let mut bitmap = PacketBitmap::new(70);
        assert!(bitmap.set(0) && bitmap.set(69));
        bitmap.grow(130);
        assert!(bitmap.contains(69), "growing keeps what was set");
        assert_eq!(bitmap.len(), 130);
        bitmap.reset(3);
        assert_eq!((bitmap.len(), bitmap.received()), (3, 0));
        assert!(!bitmap.contains(0));
    }

    #[test]
    fn first_n_complete_checks_a_prefix() {
        let mut bitmap = PacketBitmap::new(130);
        for idx in 0..128 {
            bitmap.set(idx);
        }
        assert!(bitmap.first_n_complete(64));
        assert!(bitmap.first_n_complete(128));
        assert!(!bitmap.first_n_complete(129));
        assert!(
            !bitmap.first_n_complete(131),
            "beyond the bitmap is never complete"
        );
        assert!(bitmap.first_n_complete(0));
    }

    /// An extended-ID data packet: its data starts at byte 20, not byte 8,
    /// and its ids come from the 64- and 32-bit fields.
    #[test]
    fn extended_payload_data_starts_after_the_20_byte_header() {
        #[rustfmt::skip]
        let pkt = [
            0x00, 0x00, 0x00, 0x00, 0x83, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02, // block id
            0x00, 0x01, 0x00, 0x00,                         // packet id
            0xAA, 0xBB, 0xCC,
        ];
        let packet = parse_packet_ref(&pkt).expect("parse");
        assert_eq!(packet.header.block_id, 0x0000_0001_0000_0002);
        assert_eq!(packet.header.packet_id, 0x0001_0000);
        assert_eq!(packet.header.header_len, 20);
        assert!(packet.header.extended_ids);
        assert_eq!(packet.content, GvspContent::Payload(&[0xAA, 0xBB, 0xCC]));
    }

    /// TC-05: a leader of another payload type is a value, not an error; only
    /// image types carry the image fields.
    #[test]
    fn a_non_image_leader_is_typed_not_refused() {
        let file = golden_leader(0x0003);
        let packet = parse_packet_ref(&file).expect("a file leader parses");
        let GvspContent::Leader(leader) = packet.content else {
            panic!("expected a leader");
        };
        assert_eq!(leader.payload_type, 0x0003);
        assert_eq!(leader.image, None);

        let chunked = golden_leader(0x4001);
        let packet = parse_packet_ref(&chunked).expect("parse");
        let GvspContent::Leader(leader) = packet.content else {
            panic!("expected a leader");
        };
        assert!(is_image_payload(leader.payload_type));
        assert!(has_extended_chunks(leader.payload_type));
        let image = leader.image.expect("an image leader has image fields");
        assert_eq!((image.width, image.height), (640, 480));
        assert_eq!(leader.timestamp, 0x0102_0304_0506_0708);
    }

    /// Packet formats beyond leader, payload and trailer (H.264, multipart,
    /// GenDC, ...) come back as `Unsupported` for the receiver to count.
    #[test]
    fn other_packet_formats_are_unsupported_values() {
        let pkt = [0x00, 0x00, 0x00, 0x01, 0x07, 0x00, 0x00, 0x05, 0x00];
        let packet = parse_packet_ref(&pkt).expect("parse");
        assert_eq!(
            packet.content,
            GvspContent::Unsupported {
                packet_format: 0x07
            }
        );
        assert_eq!(packet.header.packet_id, 5);
    }

    #[test]
    fn a_truncated_extended_header_is_invalid() {
        let pkt = [0x00, 0x00, 0x00, 0x00, 0x83, 0x00, 0x00, 0x00, 0x00, 0x00];
        assert!(matches!(parse_packet_ref(&pkt), Err(GvspError::Invalid(_))));
    }

    /// A device that cannot resend a packet answers with a bare header: an
    /// error status, the block and the packet id. Only the header parser can
    /// read it; as a trailer it is truncated.
    #[test]
    fn a_bare_header_parses_on_its_own() {
        let bare = [0x80, 0x0C, 0x00, 0x09, 0x02, 0x00, 0x00, 0x05];
        let header = parse_header(&bare).expect("header");
        assert_eq!(
            crate::GigeStatus::from_raw(header.status),
            crate::GigeStatus::PacketUnavailable
        );
        assert_eq!((header.block_id, header.packet_id), (9, 5));
        assert!(parse_packet_ref(&bare).is_err());
    }
}
