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

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;

use crate::U3vError;
use crate::usb::UsbTransfer;

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

/// Default USB bulk read timeout for streaming.
const STREAM_TIMEOUT: Duration = Duration::from_millis(5000);

/// Largest single payload transfer the host asks for: aravis's default
/// (`ARV_UV_STREAM_MAXIMUM_TRANSFER_SIZE_DEFAULT`, 1 MiB).
pub const DEFAULT_MAX_TRANSFER_SIZE: u32 = 1 << 20;

/// How the host reads one block: the sizes it writes to the SIRM, and reads
/// with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamLayout {
    /// Bytes of payload the device sends in one block.
    pub payload_size: u64,
    pub leader_size: u32,
    pub trailer_size: u32,
    pub payload_transfer_size: u32,
    pub payload_transfer_count: u32,
    pub final_transfer1_size: u32,
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
            return Err(U3vError::Protocol(format!(
                "a {alignment}-byte transfer alignment does not fit {max_transfer}-byte transfers"
            )));
        }
        if payload_size == 0 {
            return Err(U3vError::Protocol(
                "the device requires a zero-byte payload".into(),
            ));
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
            U3vError::Protocol(format!("{payload_size} bytes need {count} transfers"))
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
        .ok_or_else(|| U3vError::Protocol(format!("{value} bytes aligned to {alignment} overflow")))
}

/// Parsed U3V stream leader.
#[derive(Debug, Clone)]
pub struct Leader {
    /// The block this leader opens; its trailer carries the same id.
    pub block_id: u64,
    /// Payload type ([`PAYLOAD_TYPE_IMAGE`], [`PAYLOAD_TYPE_IMAGE_EXTENDED_CHUNK`], …).
    pub payload_type: u16,
    /// Device timestamp in ticks.
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
}

/// U3V stream receiver that reads blocks from a USB bulk endpoint.
pub struct U3vStream<T: UsbTransfer> {
    transport: Arc<T>,
    ep_in: u8,
    timeout: Duration,
    layout: StreamLayout,
    leader_buf: Vec<u8>,
    trailer_buf: Vec<u8>,
    payload_buf: Vec<u8>,
}

impl<T: UsbTransfer> U3vStream<T> {
    /// A receiver reading with `layout`, the transfer sizes written to the
    /// device's SIRM: it reads each transfer at exactly that size.
    pub fn new(transport: Arc<T>, ep_in: u8, layout: StreamLayout) -> Self {
        Self {
            transport,
            ep_in,
            timeout: STREAM_TIMEOUT,
            layout,
            leader_buf: vec![0u8; layout.leader_size as usize],
            trailer_buf: vec![0u8; layout.trailer_size as usize],
            payload_buf: vec![0u8; layout.payload_capacity() as usize],
        }
    }

    /// The transfer layout this stream reads with.
    pub fn layout(&self) -> &StreamLayout {
        &self.layout
    }

    /// Override the default bulk read timeout.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Receive the next complete block, waiting up to the stream timeout for
    /// it to start.
    pub fn next_frame(&mut self) -> Result<RawFrame, U3vError> {
        self.next_frame_within(self.timeout)?
            .ok_or(U3vError::Timeout)
    }

    /// Like [`next_frame`](Self::next_frame), but `Ok(None)` when no block
    /// starts within `idle`: the stream is waiting — in trigger mode, say —
    /// not broken. Once a leader arrives the rest of the block gets the full
    /// timeout. A reader polling with this can stop between blocks.
    ///
    /// A transfer that is not a leader where a leader is due — the tail of
    /// a block whose start was missed — is skipped.
    pub fn next_frame_within(&mut self, idle: Duration) -> Result<Option<RawFrame>, U3vError> {
        let leader = loop {
            let n = match self
                .transport
                .bulk_read(self.ep_in, &mut self.leader_buf, idle)
            {
                Ok(n) => n,
                Err(U3vError::Timeout) => return Ok(None),
                Err(e) => return Err(e),
            };
            let transfer = &self.leader_buf[..n];
            if transfer.len() >= 4 && transfer[..4] == LEADER_PREFIX.to_le_bytes() {
                break parse_leader(transfer)?;
            }
            tracing::debug!(bytes = n, "skipping a transfer that is not a leader");
        };
        let received = self.read_payload()?;
        let trailer = self.read_trailer()?;
        if trailer.block_id != leader.block_id {
            return Err(U3vError::Protocol(format!(
                "block {} closed by the trailer of block {}",
                leader.block_id, trailer.block_id
            )));
        }
        let valid = (trailer.valid_payload_size as usize).min(received);
        Ok(Some(RawFrame {
            payload: Bytes::copy_from_slice(&self.payload_buf[..valid]),
            leader,
            trailer,
        }))
    }

    /// Read the payload's transfers at the sizes in the layout. A short
    /// transfer is the device ending the payload early; the trailer follows
    /// and says how much of it is valid.
    fn read_payload(&mut self) -> Result<usize, U3vError> {
        let layout = self.layout;
        let transfers = std::iter::repeat_n(
            layout.payload_transfer_size,
            layout.payload_transfer_count as usize,
        )
        .chain([layout.final_transfer1_size, layout.final_transfer2_size])
        .filter(|&size| size > 0);

        let mut offset = 0usize;
        for size in transfers {
            let end = offset + size as usize;
            let n = self.transport.bulk_read(
                self.ep_in,
                &mut self.payload_buf[offset..end],
                self.timeout,
            )?;
            offset += n;
            if n < size as usize {
                break;
            }
        }
        Ok(offset)
    }

    fn read_trailer(&mut self) -> Result<Trailer, U3vError> {
        let n = self
            .transport
            .bulk_read(self.ep_in, &mut self.trailer_buf, self.timeout)?;
        parse_trailer(&self.trailer_buf[..n])
    }
}

/// A raw frame as received from the U3V stream, before conversion to
/// the transport-agnostic `Frame` type.
#[derive(Debug)]
pub struct RawFrame {
    /// Leader containing image metadata.
    pub leader: Leader,
    /// The valid payload bytes.
    pub payload: Bytes,
    /// Trailer with status and block ID.
    pub trailer: Trailer,
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
        return Err(U3vError::Protocol(format!(
            "leader too short: {} bytes, need at least {GENERIC_LEADER_SIZE}",
            buf.len()
        )));
    }

    let prefix = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if prefix != LEADER_PREFIX {
        return Err(U3vError::Protocol(format!(
            "bad leader prefix: {prefix:#010x}, expected {LEADER_PREFIX:#010x}"
        )));
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
            return Err(U3vError::Protocol(format!(
                "image leader too short: {} bytes, need {IMAGE_LEADER_SIZE}",
                buf.len()
            )));
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
        return Err(U3vError::Protocol(format!(
            "trailer too short: {} bytes, need at least {MIN_TRAILER_SIZE}",
            buf.len()
        )));
    }

    let prefix = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if prefix != TRAILER_PREFIX {
        return Err(U3vError::Protocol(format!(
            "bad trailer prefix: {prefix:#010x}, expected {TRAILER_PREFIX:#010x}"
        )));
    }

    Ok(Trailer {
        block_id: u64::from_le_bytes(buf[8..16].try_into().unwrap()),
        status: u16::from_le_bytes([buf[16], buf[17]]),
        valid_payload_size: u64::from_le_bytes(buf[20..28].try_into().unwrap()),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usb::MockUsbTransfer;

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

    /// An idle stream is waiting, not broken, so a reader can poll it and
    /// stop between frames (Codex review of #179).
    #[test]
    fn an_idle_stream_is_waiting_not_broken() {
        let mut stream = U3vStream::new(Arc::new(Idle), 0x82, layout(4096, 4096));
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
        let ep_in = 0x82;
        let payload_size = 320 * 240;

        mock.enqueue_read(ep_in, build_leader(7, 320, 240, 0x0108_0001));
        mock.enqueue_read(ep_in, vec![0x80; payload_size]);
        mock.enqueue_read(ep_in, build_trailer(7, 0, payload_size as u64));

        let mut stream = U3vStream::new(
            Arc::clone(&mock),
            ep_in,
            layout(payload_size as u64, 1 << 20),
        );
        let frame = stream.next_frame().unwrap();

        assert_eq!(frame.leader.block_id, 7);
        assert_eq!((frame.leader.width, frame.leader.height), (320, 240));
        assert_eq!(frame.leader.timestamp, 7000);
        assert_eq!(frame.payload.len(), payload_size);
        assert_eq!(frame.trailer.status, 0);
    }

    /// Each transfer is read at the size the layout gives it.
    #[test]
    fn the_payload_is_read_in_the_layouts_transfers() {
        let mock = Arc::new(MockUsbTransfer::new());
        let ep_in = 0x82;
        let stream_layout = layout(1000, 256); // 3 × 256, final 232
        assert_eq!(stream_layout.payload_transfer_count, 3);
        assert_eq!(stream_layout.final_transfer1_size, 232);

        mock.enqueue_read(ep_in, build_leader(1, 25, 40, 0x0108_0001));
        for fill in 0..3u8 {
            mock.enqueue_read(ep_in, vec![fill; 256]);
        }
        mock.enqueue_read(ep_in, vec![3; 232]);
        mock.enqueue_read(ep_in, build_trailer(1, 0, 1000));

        let mut stream = U3vStream::new(Arc::clone(&mock), ep_in, stream_layout);
        let frame = stream.next_frame().unwrap();
        assert_eq!(frame.payload.len(), 1000);
        assert!(frame.payload[..256].iter().all(|&b| b == 0));
        assert!(frame.payload[768..].iter().all(|&b| b == 3));
    }

    /// A short transfer ends the payload; the trailer says how much is valid.
    #[test]
    fn a_short_payload_ends_at_the_trailer() {
        let mock = Arc::new(MockUsbTransfer::new());
        let ep_in = 0x82;
        mock.enqueue_read(ep_in, build_leader(2, 25, 40, 0x0108_0001));
        mock.enqueue_read(ep_in, vec![9; 100]);
        mock.enqueue_read(ep_in, build_trailer(2, 0xA100, 100));

        let mut stream = U3vStream::new(Arc::clone(&mock), ep_in, layout(1000, 256));
        let frame = stream.next_frame().unwrap();
        assert_eq!(frame.payload.len(), 100);
        assert_eq!(frame.trailer.status, 0xA100);
    }

    #[test]
    fn a_stray_transfer_before_the_leader_is_skipped() {
        let mock = Arc::new(MockUsbTransfer::new());
        let ep_in = 0x82;
        mock.enqueue_read(ep_in, vec![0xEE; 64]);
        mock.enqueue_read(ep_in, build_leader(3, 8, 8, 0x0108_0001));
        mock.enqueue_read(ep_in, vec![1; 64]);
        mock.enqueue_read(ep_in, build_trailer(3, 0, 64));

        let mut stream = U3vStream::new(Arc::clone(&mock), ep_in, layout(64, 256));
        assert_eq!(stream.next_frame().unwrap().leader.block_id, 3);
    }

    #[test]
    fn a_trailer_for_another_block_is_an_error() {
        let mock = Arc::new(MockUsbTransfer::new());
        let ep_in = 0x82;
        mock.enqueue_read(ep_in, build_leader(4, 8, 8, 0x0108_0001));
        mock.enqueue_read(ep_in, vec![1; 64]);
        mock.enqueue_read(ep_in, build_trailer(5, 0, 64));

        let mut stream = U3vStream::new(Arc::clone(&mock), ep_in, layout(64, 256));
        assert!(matches!(
            stream.next_frame(),
            Err(U3vError::Protocol(ref m)) if m.contains("block 4")
        ));
    }
}
