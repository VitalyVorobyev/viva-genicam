//! GVSP datagrams as a camera transmits them, and a deterministic script for
//! damaging them.
//!
//! The byte layouts are written out from the GigE Vision field tables with this
//! crate's **own** constants, never the client's (ADR-0019): a fake that
//! borrowed `viva-gige`'s header length would agree with any mistake in it.
//! The two layouts:
//!
//! | Bytes | Standard header (8)        | Extended-ID header (20)          |
//! |-------|----------------------------|----------------------------------|
//! | 0–1   | status                     | status                           |
//! | 2–3   | block id (16 bits)         | flags (0)                        |
//! | 4     | packet format              | `0x80` \| packet format          |
//! | 5–7   | packet id (24 bits)        | 0 (unused in extended mode)      |
//! | 8–15  | —                          | block id (64 bits)               |
//! | 16–19 | —                          | packet id (32 bits)              |
//!
//! The extended layout's zero bytes 2–3 and 5–7 are what a LUCID TRT009S-E put
//! on the wire (the capture pinned in `viva-gige`'s parser tests); aravis's
//! `ArvGvspExtendedHeader` reads the same fields.
//!
//! `GevSCPSPacketSize` counts the IP datagram, so a data packet carries
//! `packet_size − 20 (IPv4) − 8 (UDP) − header` bytes of image: 1 464 at 1 500
//! with standard ids, 1 452 with extended ids.
//!
//! [`PacketScript`] reorders, repeats or drops datagrams by block and packet
//! id, never by time, so a test that uses it behaves the same on every runner.

/// Standard GVSP header length.
pub const STANDARD_HEADER_LEN: usize = 8;
/// Extended-ID GVSP header length (GigE Vision 2.0).
pub const EXTENDED_HEADER_LEN: usize = 20;
/// IPv4 header counted by `GevSCPSPacketSize`.
const IPV4_HEADER_LEN: usize = 20;
/// UDP header counted by `GevSCPSPacketSize`.
const UDP_HEADER_LEN: usize = 8;

/// Packet format: data leader.
const FORMAT_LEADER: u8 = 0x01;
/// Packet format: data trailer.
const FORMAT_TRAILER: u8 = 0x02;
/// Packet format: data payload (generic).
const FORMAT_PAYLOAD: u8 = 0x03;
/// Bit 7 of the packet-format byte: extended ids.
const EXTENDED_ID_FLAG: u8 = 0x80;

/// Payload type: image.
pub const PAYLOAD_TYPE_IMAGE: u16 = 0x0001;
/// Payload type: image with extended chunk data (bit 14 set on image).
pub const PAYLOAD_TYPE_IMAGE_EXTENDED_CHUNK: u16 = 0x4001;
/// Payload type: chunk data, a block made only of chunks.
pub const PAYLOAD_TYPE_CHUNK_DATA: u16 = 0x0004;
/// Bit 14 of the payload type: extended chunk data.
const EXTENDED_CHUNK_FLAG: u16 = 0x4000;
/// Bits of the payload type that name the type.
const PAYLOAD_TYPE_MASK: u16 = 0x3FFF;
/// A chunk's tag: its ID, then its data length, both 32-bit big-endian.
const CHUNK_TAG_LEN: usize = 8;

/// Lay `chunks` out as a GigE Vision chunk payload, in order: each chunk's
/// data, then its tag — chunk ID and data length, 32-bit big-endian each.
///
/// Written from the layout itself, with this crate's own constants; it never
/// calls the client's parser (ADR-0019). A receiver reads it from the end.
/// For an image with chunks (`0x4001`) the image is the first chunk.
pub fn chunk_payload(chunks: &[(u32, &[u8])]) -> Vec<u8> {
    let total = chunks
        .iter()
        .map(|(_, data)| data.len() + CHUNK_TAG_LEN)
        .sum();
    let mut buf = Vec::with_capacity(total);
    for (id, data) in chunks {
        buf.extend_from_slice(data);
        buf.extend_from_slice(&id.to_be_bytes());
        buf.extend_from_slice(&(data.len() as u32).to_be_bytes());
    }
    buf
}

/// Bytes an image of this format occupies on the wire, by the fake's own rule.
///
/// Bits 23–16 of a standard PFNC code are the pixel's bit depth, so a packed
/// format such as `Mono12Packed` (12 bits) sends `ceil(w × h × 12 / 8)` bytes
/// — not `w × h × bytes-per-pixel`, which no whole number satisfies. Vendor
/// codes (bit 31 set) keep the fake's historical one byte per pixel; the EVS
/// test streams were written around that.
pub fn image_size(pixel_format: u32, width: u32, height: u32) -> usize {
    let pixels = width as usize * height as usize;
    if pixel_format & 0x8000_0000 != 0 {
        return pixels;
    }
    let bits = ((pixel_format >> 16) & 0xFF) as usize;
    if bits == 0 {
        return pixels;
    }
    (pixels * bits).div_ceil(8)
}

/// What one block looks like on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockSpec {
    /// Block id; truncated to 16 bits unless `extended_ids`.
    pub block_id: u64,
    /// `GevSCPSPacketSize` in force.
    pub packet_size: u32,
    /// Send 20-byte extended-ID headers.
    pub extended_ids: bool,
    /// Payload type in the leader and trailer.
    pub payload_type: u16,
    /// PFNC code in the leader.
    pub pixel_format: u32,
    /// Size X in the leader.
    pub width: u32,
    /// Size Y in the leader.
    pub height: u32,
    /// Device timestamp in the leader.
    pub timestamp: u64,
    /// Status word of the trailer's header; 0 is success.
    pub trailer_status: u16,
    /// Size Y in the trailer: the lines the block actually delivered.
    pub size_y: u32,
    /// Chunk layout ID the trailer reports when the payload type carries the
    /// extended chunk flag.
    pub chunk_layout_id: u32,
    /// Padding X in the leader: bytes after every line of the image.
    pub padding_x: u16,
}

impl BlockSpec {
    /// An image block with standard ids and a successful trailer.
    pub fn image(
        block_id: u64,
        width: u32,
        height: u32,
        pixel_format: u32,
        packet_size: u32,
    ) -> Self {
        Self {
            block_id,
            packet_size,
            extended_ids: false,
            payload_type: PAYLOAD_TYPE_IMAGE,
            pixel_format,
            width,
            height,
            timestamp: 0,
            trailer_status: 0,
            size_y: height,
            chunk_layout_id: 0,
            padding_x: 0,
        }
    }

    /// A chunk-data block (`0x0004`): no image, so no geometry.
    pub fn chunk_data(block_id: u64, packet_size: u32) -> Self {
        Self {
            payload_type: PAYLOAD_TYPE_CHUNK_DATA,
            pixel_format: 0,
            width: 0,
            height: 0,
            size_y: 0,
            ..Self::image(block_id, 0, 0, 0, packet_size)
        }
    }

    /// Whether the leader carries image geometry.
    fn is_image(&self) -> bool {
        self.payload_type & PAYLOAD_TYPE_MASK == PAYLOAD_TYPE_IMAGE
    }

    /// Header length this block's packets carry.
    pub fn header_len(&self) -> usize {
        if self.extended_ids {
            EXTENDED_HEADER_LEN
        } else {
            STANDARD_HEADER_LEN
        }
    }

    /// Image bytes one data packet carries.
    pub fn payload_stride(&self) -> usize {
        (self.packet_size as usize)
            .saturating_sub(IPV4_HEADER_LEN + UDP_HEADER_LEN + self.header_len())
    }
}

/// Which part of a block a datagram is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketKind {
    /// Data leader (packet id 0).
    Leader,
    /// Data payload (packet ids 1..=n).
    Payload,
    /// Data trailer (packet id n + 1).
    Trailer,
}

/// One GVSP datagram, labelled for scripting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datagram {
    /// Block the datagram belongs to.
    pub block_id: u64,
    /// Its packet id.
    pub packet_id: u32,
    /// Leader, payload or trailer.
    pub kind: PacketKind,
    /// The UDP payload exactly as sent.
    pub bytes: Vec<u8>,
}

fn put_header(buf: &mut Vec<u8>, spec: &BlockSpec, status: u16, format: u8, packet_id: u32) {
    buf.extend_from_slice(&status.to_be_bytes());
    if spec.extended_ids {
        buf.extend_from_slice(&0u16.to_be_bytes()); // flags
        buf.push(EXTENDED_ID_FLAG | format);
        buf.extend_from_slice(&[0, 0, 0]); // unused 24-bit packet id
        buf.extend_from_slice(&spec.block_id.to_be_bytes());
        buf.extend_from_slice(&packet_id.to_be_bytes());
    } else {
        buf.extend_from_slice(&(spec.block_id as u16).to_be_bytes());
        buf.push(format);
        let id = packet_id.to_be_bytes();
        buf.extend_from_slice(&id[1..4]);
    }
}

/// The leader: header, then reserved(2) | payload type(2) | timestamp(8),
/// then for an image pixel format(4) | size x(4) | size y(4) | offset x(4) |
/// offset y(4) | padding x(2) | padding y(2). A chunk-data leader stops after
/// the timestamp: it has no geometry to report.
pub fn leader(spec: &BlockSpec) -> Vec<u8> {
    let mut buf = Vec::with_capacity(spec.header_len() + 36);
    put_header(&mut buf, spec, 0, FORMAT_LEADER, 0);
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&spec.payload_type.to_be_bytes());
    buf.extend_from_slice(&spec.timestamp.to_be_bytes());
    if !spec.is_image() {
        return buf;
    }
    buf.extend_from_slice(&spec.pixel_format.to_be_bytes());
    buf.extend_from_slice(&spec.width.to_be_bytes());
    buf.extend_from_slice(&spec.height.to_be_bytes());
    buf.extend_from_slice(&0u32.to_be_bytes()); // offset x
    buf.extend_from_slice(&0u32.to_be_bytes()); // offset y
    buf.extend_from_slice(&spec.padding_x.to_be_bytes()); // padding x
    buf.extend_from_slice(&0u16.to_be_bytes()); // padding y
    buf
}

/// A data packet: header, then image bytes.
pub fn payload(spec: &BlockSpec, packet_id: u32, data: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(spec.header_len() + data.len());
    put_header(&mut buf, spec, 0, FORMAT_PAYLOAD, packet_id);
    buf.extend_from_slice(data);
    buf
}

/// The trailer: header (carrying `trailer_status`), then reserved(2) |
/// payload type(2) | size y(4) — for chunk data, the payload length in that
/// field — and, with the extended chunk flag, chunk payload length(4) | chunk
/// layout id(4). The chunks themselves travel in the data packets.
pub fn trailer(spec: &BlockSpec, packet_id: u32, payload_len: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(spec.header_len() + 16);
    put_header(
        &mut buf,
        spec,
        spec.trailer_status,
        FORMAT_TRAILER,
        packet_id,
    );
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&spec.payload_type.to_be_bytes());
    let size_field = if spec.payload_type & PAYLOAD_TYPE_MASK == PAYLOAD_TYPE_CHUNK_DATA {
        payload_len as u32
    } else {
        spec.size_y
    };
    buf.extend_from_slice(&size_field.to_be_bytes());
    if spec.payload_type & EXTENDED_CHUNK_FLAG != 0 {
        buf.extend_from_slice(&(payload_len as u32).to_be_bytes());
        buf.extend_from_slice(&spec.chunk_layout_id.to_be_bytes());
    }
    buf
}

/// A GVSP packet that is a header and nothing else, carrying `status`: how
/// the fake answers a resend request it cannot serve. The packet format is
/// generic payload, the block and packet ids those of the first packet asked
/// for.
pub fn header_only(block_id: u64, extended_ids: bool, packet_id: u32, status: u16) -> Vec<u8> {
    let mut spec = BlockSpec::image(block_id, 0, 0, 0, 0);
    spec.extended_ids = extended_ids;
    let mut buf = Vec::with_capacity(spec.header_len());
    put_header(&mut buf, &spec, status, FORMAT_PAYLOAD, packet_id);
    buf
}

/// Every datagram of one block in transmission order: the leader, `payload`
/// split at the block's stride — an image, or a [`chunk_payload`] — and the
/// trailer.
pub fn block_datagrams(spec: &BlockSpec, data: &[u8]) -> Vec<Datagram> {
    let stride = spec.payload_stride().max(1);
    let mut out = Vec::with_capacity(data.len() / stride + 3);
    out.push(Datagram {
        block_id: spec.block_id,
        packet_id: 0,
        kind: PacketKind::Leader,
        bytes: leader(spec),
    });
    let mut packet_id = 1u32;
    for piece in data.chunks(stride) {
        out.push(Datagram {
            block_id: spec.block_id,
            packet_id,
            kind: PacketKind::Payload,
            bytes: payload(spec, packet_id, piece),
        });
        packet_id += 1;
    }
    out.push(Datagram {
        block_id: spec.block_id,
        packet_id,
        kind: PacketKind::Trailer,
        bytes: trailer(spec, packet_id, data.len()),
    });
    out
}

/// One deterministic change to the datagrams of a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptStep {
    /// Never send packet `packet` of `block`.
    Drop {
        /// Block id.
        block: u64,
        /// Packet id.
        packet: u32,
    },
    /// Send nothing of `block`.
    DropBlock {
        /// Block id.
        block: u64,
    },
    /// Send packet `packet` of `block` twice in a row.
    Duplicate {
        /// Block id.
        block: u64,
        /// Packet id.
        packet: u32,
    },
    /// Send packet `packet` of `block` after `by` more of the block's datagrams.
    Delay {
        /// Block id.
        block: u64,
        /// Packet id.
        packet: u32,
        /// How many datagrams overtake it.
        by: usize,
    },
    /// Send the trailer of `block` just before its last payload packet.
    TrailerBeforeLastPayload {
        /// Block id.
        block: u64,
    },
    /// Send the leader of `block` again after its payload packet `after`.
    RepeatLeader {
        /// Block id.
        block: u64,
        /// Payload packet id the repeat follows.
        after: u32,
    },
    /// Alternate the datagrams of `block` with those of the next block.
    ///
    /// Only the live sender acts on this; [`PacketScript::apply`] sees one
    /// block at a time. Tests driving datagrams themselves call [`interleave`].
    Interleave {
        /// Block id of the first of the two blocks.
        block: u64,
    },
}

/// A deterministic list of [`ScriptStep`]s, keyed by block and packet id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PacketScript {
    steps: Vec<ScriptStep>,
}

impl PacketScript {
    /// An empty script: every datagram is sent once, in order.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a step.
    pub fn step(mut self, step: ScriptStep) -> Self {
        self.steps.push(step);
        self
    }

    /// The steps, in the order they were added.
    pub fn steps(&self) -> &[ScriptStep] {
        &self.steps
    }

    /// Whether the script interleaves `block` with the block after it.
    pub fn interleaves(&self, block: u64) -> bool {
        self.steps
            .iter()
            .any(|s| matches!(s, ScriptStep::Interleave { block: b } if *b == block))
    }

    /// Apply every step that names the block of `datagrams`.
    ///
    /// Steps run in a fixed order whatever order they were added in: drops,
    /// then the repeated leader, then the early trailer, then delays, then
    /// duplicates.
    pub fn apply(&self, mut datagrams: Vec<Datagram>) -> Vec<Datagram> {
        let Some(block) = datagrams.first().map(|d| d.block_id) else {
            return datagrams;
        };
        let mine = || self.steps.iter().filter(move |s| step_block(s) == block);

        if mine().any(|s| matches!(s, ScriptStep::DropBlock { .. })) {
            return Vec::new();
        }
        for step in mine() {
            if let ScriptStep::Drop { packet, .. } = *step {
                datagrams.retain(|d| d.packet_id != packet);
            }
        }
        for step in mine() {
            if let ScriptStep::RepeatLeader { after, .. } = *step
                && let Some(leader) = datagrams
                    .iter()
                    .find(|d| d.kind == PacketKind::Leader)
                    .cloned()
                && let Some(pos) = datagrams
                    .iter()
                    .position(|d| d.kind == PacketKind::Payload && d.packet_id == after)
            {
                datagrams.insert(pos + 1, leader);
            }
        }
        for step in mine() {
            if let ScriptStep::TrailerBeforeLastPayload { .. } = *step
                && let Some(t) = datagrams.iter().position(|d| d.kind == PacketKind::Trailer)
            {
                let trailer = datagrams.remove(t);
                let last = datagrams
                    .iter()
                    .rposition(|d| d.kind == PacketKind::Payload)
                    .unwrap_or(datagrams.len());
                datagrams.insert(last, trailer);
            }
        }
        for step in mine() {
            if let ScriptStep::Delay { packet, by, .. } = *step
                && let Some(pos) = datagrams.iter().position(|d| d.packet_id == packet)
            {
                let moved = datagrams.remove(pos);
                let to = (pos + by).min(datagrams.len());
                datagrams.insert(to, moved);
            }
        }
        for step in mine() {
            if let ScriptStep::Duplicate { packet, .. } = *step
                && let Some(pos) = datagrams.iter().position(|d| d.packet_id == packet)
            {
                let copy = datagrams[pos].clone();
                datagrams.insert(pos + 1, copy);
            }
        }
        datagrams
    }
}

fn step_block(step: &ScriptStep) -> u64 {
    match *step {
        ScriptStep::Drop { block, .. }
        | ScriptStep::DropBlock { block }
        | ScriptStep::Duplicate { block, .. }
        | ScriptStep::Delay { block, .. }
        | ScriptStep::TrailerBeforeLastPayload { block }
        | ScriptStep::RepeatLeader { block, .. }
        | ScriptStep::Interleave { block } => block,
    }
}

/// Alternate two blocks' datagrams, `a` first; the longer one's tail follows.
pub fn interleave(a: Vec<Datagram>, b: Vec<Datagram>) -> Vec<Datagram> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    let mut a = a.into_iter();
    let mut b = b.into_iter();
    loop {
        match (a.next(), b.next()) {
            (None, None) => break,
            (x, y) => {
                out.extend(x);
                out.extend(y);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Extended-ID leader bytes written out field by field, independently of
    /// [`leader`] (ADR-0019 item 2).
    #[test]
    fn extended_leader_matches_the_field_table() {
        let mut spec = BlockSpec::image(0x0102_0304_0506_0708, 2, 3, 0x0108_0001, 1500);
        spec.extended_ids = true;
        spec.timestamp = 0x1122_3344_5566_7788;
        #[rustfmt::skip]
        let expected: Vec<u8> = vec![
            0x00, 0x00,                                     // status
            0x00, 0x00,                                     // flags
            0x81,                                           // EI | leader
            0x00, 0x00, 0x00,                               // unused packet id
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, // block id
            0x00, 0x00, 0x00, 0x00,                         // packet id 0
            0x00, 0x00,                                     // reserved
            0x00, 0x01,                                     // payload type
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, // timestamp
            0x01, 0x08, 0x00, 0x01,                         // Mono8
            0x00, 0x00, 0x00, 0x02,                         // size x
            0x00, 0x00, 0x00, 0x03,                         // size y
            0x00, 0x00, 0x00, 0x00,                         // offset x
            0x00, 0x00, 0x00, 0x00,                         // offset y
            0x00, 0x00, 0x00, 0x00,                         // padding x, y
        ];
        assert_eq!(leader(&spec), expected);
    }

    #[test]
    fn extended_payload_carries_a_32_bit_packet_id_at_offset_16() {
        let mut spec = BlockSpec::image(9, 2, 3, 0x0108_0001, 1500);
        spec.extended_ids = true;
        let pkt = payload(&spec, 0x0001_0203, &[0xAA, 0xBB]);
        assert_eq!(pkt.len(), 22);
        assert_eq!(pkt[4], 0x83);
        assert_eq!(&pkt[8..16], &9u64.to_be_bytes());
        assert_eq!(&pkt[16..20], &[0x00, 0x01, 0x02, 0x03]);
        assert_eq!(&pkt[20..], &[0xAA, 0xBB]);
    }

    #[test]
    fn a_header_only_packet_carries_its_status() {
        assert_eq!(
            header_only(0x1234, false, 0x0001_0203, 0x800C),
            vec![0x80, 0x0C, 0x12, 0x34, 0x03, 0x01, 0x02, 0x03]
        );
        let ext = header_only(7, true, 9, 0x800C);
        assert_eq!(ext.len(), 20);
        assert_eq!(&ext[..5], &[0x80, 0x0C, 0x00, 0x00, 0x83]);
        assert_eq!(&ext[8..16], &7u64.to_be_bytes());
        assert_eq!(&ext[16..20], &9u32.to_be_bytes());
    }

    #[test]
    fn the_stride_follows_the_header_length() {
        let mut spec = BlockSpec::image(1, 8, 8, 0x0108_0001, 1500);
        assert_eq!(spec.payload_stride(), 1464);
        spec.extended_ids = true;
        assert_eq!(spec.payload_stride(), 1452);
    }

    #[test]
    fn packed_formats_are_sized_by_bit_depth() {
        // Mono12Packed: 12 bits, so four pixels are six bytes.
        assert_eq!(image_size(0x010C_0006, 4, 1), 6);
        assert_eq!(image_size(0x010C_0006, 640, 480), 640 * 480 * 3 / 2);
        assert_eq!(image_size(0x0108_0001, 640, 480), 640 * 480);
        assert_eq!(image_size(0x0110_0007, 2, 2), 8); // Mono16
        assert_eq!(image_size(0x0218_0014, 2, 2), 12); // RGB8
        assert_eq!(image_size(0x8110_0E30, 16, 4), 64); // vendor: unchanged
    }

    fn ids(datagrams: &[Datagram]) -> Vec<u32> {
        datagrams.iter().map(|d| d.packet_id).collect()
    }

    fn sample() -> Vec<Datagram> {
        let spec = BlockSpec::image(5, 10, 1, 0x0108_0001, 28 + 8 + 4);
        block_datagrams(&spec, &[0u8; 10])
    }

    #[test]
    fn a_block_is_leader_payloads_trailer() {
        // 10 bytes at a 4-byte stride: three payload packets.
        assert_eq!(ids(&sample()), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn script_steps_act_by_packet_id() {
        let d = || sample();
        let s = |step| PacketScript::new().step(step);
        assert_eq!(
            ids(&s(ScriptStep::Drop {
                block: 5,
                packet: 2
            })
            .apply(d())),
            vec![0, 1, 3, 4]
        );
        assert!(s(ScriptStep::DropBlock { block: 5 }).apply(d()).is_empty());
        assert_eq!(
            ids(&s(ScriptStep::Duplicate {
                block: 5,
                packet: 1
            })
            .apply(d())),
            vec![0, 1, 1, 2, 3, 4]
        );
        assert_eq!(
            ids(&s(ScriptStep::Delay {
                block: 5,
                packet: 1,
                by: 2
            })
            .apply(d())),
            vec![0, 2, 3, 1, 4]
        );
        assert_eq!(
            ids(&s(ScriptStep::TrailerBeforeLastPayload { block: 5 }).apply(d())),
            vec![0, 1, 2, 4, 3]
        );
        assert_eq!(
            ids(&s(ScriptStep::RepeatLeader { block: 5, after: 2 }).apply(d())),
            vec![0, 1, 2, 0, 3, 4]
        );
        // Another block's steps leave this one alone.
        assert_eq!(
            ids(&s(ScriptStep::Drop {
                block: 6,
                packet: 2
            })
            .apply(d())),
            vec![0, 1, 2, 3, 4]
        );
    }

    #[test]
    fn interleave_alternates_two_blocks() {
        let a = sample();
        let mut b = sample();
        for d in &mut b {
            d.block_id = 6;
        }
        let merged = interleave(a, b);
        let order: Vec<(u64, u32)> = merged.iter().map(|d| (d.block_id, d.packet_id)).collect();
        assert_eq!(&order[..4], &[(5, 0), (6, 0), (5, 1), (6, 1)]);
        assert_eq!(merged.len(), 10);
    }

    /// The chunk layout, byte for byte from the field description: each
    /// chunk's data, then its 32-bit big-endian ID and length.
    #[test]
    fn chunk_payload_matches_the_layout() {
        let payload = chunk_payload(&[(1, &[0xAA, 0xBB]), (0xA5A5_0100, &[1, 2, 3, 4])]);
        #[rustfmt::skip]
        let expected = [
            0xAA, 0xBB,
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02,
            0x01, 0x02, 0x03, 0x04,
            0xA5, 0xA5, 0x01, 0x00, 0x00, 0x00, 0x00, 0x04,
        ];
        assert_eq!(payload, expected);
    }

    /// With the chunk flag, the trailer adds the chunk payload length and the
    /// layout ID after its 8 bytes; a chunk-data leader stops at the
    /// timestamp, with no geometry.
    #[test]
    fn chunk_flavoured_leader_and_trailer_match_the_field_table() {
        let mut spec = BlockSpec::image(9, 4, 2, 0x0108_0001, 576);
        spec.payload_type = PAYLOAD_TYPE_IMAGE_EXTENDED_CHUNK;
        spec.chunk_layout_id = 0x1234_5678;
        let t = trailer(&spec, 3, 44);
        assert_eq!(
            &t[8..],
            &[
                0x00, 0x00, 0x40, 0x01, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x2C, 0x12, 0x34,
                0x56, 0x78
            ]
        );

        let spec = BlockSpec::chunk_data(9, 576);
        assert_eq!(
            leader(&spec).len(),
            8 + 12,
            "reserved, payload type, timestamp"
        );
        let t = trailer(&spec, 3, 44);
        assert_eq!(
            &t[8..],
            &[0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x2C],
            "a chunk-data trailer reports the payload length where an image has Size Y"
        );
    }
}
