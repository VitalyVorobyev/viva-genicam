//! USB3 Vision streaming protocol.
//!
//! U3V streaming uses USB bulk transfers with a leader/payload/trailer
//! structure. Unlike GigE Vision (GVSP over UDP), USB bulk transfers are
//! reliable and ordered — no packet reassembly, bitmap tracking, or resend
//! logic is needed.
//!
//! Frame structure:
//! ```text
//! ┌─────────┐   ┌─────────────┐   ┌──────────┐
//! │ Leader  │ → │ Payload(s)  │ → │ Trailer  │
//! │ (meta)  │   │ (image data)│   │ (status) │
//! └─────────┘   └─────────────┘   └──────────┘
//! ```

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;

use crate::U3vError;
use crate::usb::UsbTransfer;

/// U3V stream leader prefix: "U3VL" in little-endian.
const LEADER_PREFIX: u32 = 0x4C56_3355;
/// U3V stream trailer prefix: "U3VT" in little-endian.
const TRAILER_PREFIX: u32 = 0x5456_3355;

/// Minimum leader size (prefix + fixed fields).
const MIN_LEADER_SIZE: usize = 36;
/// Minimum trailer size (prefix + fixed fields).
const MIN_TRAILER_SIZE: usize = 16;

/// Default USB bulk read timeout for streaming.
const STREAM_TIMEOUT: Duration = Duration::from_millis(5000);

/// Parsed U3V stream leader (image metadata).
#[derive(Debug, Clone)]
pub struct Leader {
    /// Payload type (0x0001 = image, 0x0002 = image extended, 0x4001 = chunk).
    pub payload_type: u16,
    /// Device timestamp in ticks.
    pub timestamp: u64,
    /// PFNC pixel format code.
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
    /// Status of the acquired frame (0 = success).
    pub status: u32,
    /// Block ID for this frame.
    pub block_id: u64,
    /// Actual number of valid payload bytes.
    pub valid_payload_size: u64,
}

/// U3V stream receiver that reads frames from a USB bulk endpoint.
pub struct U3vStream<T: UsbTransfer> {
    transport: Arc<T>,
    ep_in: u8,
    timeout: Duration,
    leader_buf: Vec<u8>,
    trailer_buf: Vec<u8>,
    payload_buf: Vec<u8>,
    /// Configured payload size per frame (from SIRM). The device sends
    /// exactly this many bytes of payload data, potentially across
    /// multiple USB bulk transfers.
    payload_size: usize,
}

impl<T: UsbTransfer> U3vStream<T> {
    /// Create a new stream receiver.
    ///
    /// `max_leader_size`, `max_trailer_size` and `payload_size` come
    /// from the SIRM registers. `payload_size` is the exact number of
    /// payload bytes per frame (configured in the SIRM).
    pub fn new(
        transport: Arc<T>,
        ep_in: u8,
        max_leader_size: usize,
        max_trailer_size: usize,
        payload_size: usize,
    ) -> Self {
        Self {
            transport,
            ep_in,
            timeout: STREAM_TIMEOUT,
            leader_buf: vec![0u8; max_leader_size],
            trailer_buf: vec![0u8; max_trailer_size],
            payload_buf: vec![0u8; payload_size],
            payload_size,
        }
    }

    /// Override the default bulk read timeout.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Receive the next complete frame.
    ///
    /// Blocks until a full leader + payload + trailer sequence is read
    /// from the stream endpoint. Returns the parsed leader, raw payload,
    /// and trailer.
    pub fn next_frame(&mut self) -> Result<RawFrame, U3vError> {
        let leader = self.read_leader()?;
        let payload = self.read_payload()?;
        let trailer = self.read_trailer()?;

        Ok(RawFrame {
            leader,
            payload,
            trailer,
        })
    }

    fn read_leader(&mut self) -> Result<Leader, U3vError> {
        let n = self
            .transport
            .bulk_read(self.ep_in, &mut self.leader_buf, self.timeout)?;
        parse_leader(&self.leader_buf[..n])
    }

    /// Read the full payload, looping over multiple USB bulk transfers
    /// if the payload is larger than a single transfer.
    fn read_payload(&mut self) -> Result<Bytes, U3vError> {
        let mut offset = 0;
        while offset < self.payload_size {
            let n = self.transport.bulk_read(
                self.ep_in,
                &mut self.payload_buf[offset..self.payload_size],
                self.timeout,
            )?;
            if n == 0 {
                return Err(U3vError::Protocol(format!(
                    "zero-length bulk read at payload offset {offset}/{}",
                    self.payload_size
                )));
            }
            offset += n;
        }
        Ok(Bytes::copy_from_slice(
            &self.payload_buf[..self.payload_size],
        ))
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
    /// Raw pixel payload.
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
/// [0..4]   prefix        0x4C563355 ("U3VL")
/// [4..6]   reserved
/// [6..8]   leader_size   (total size in bytes)
/// [8..10]  reserved
/// [10..12] payload_type
/// [12..20] timestamp     (device ticks)
/// [20..24] pixel_format  (PFNC code)
/// [24..28] width
/// [28..32] height
/// [32..36] x_offset
/// [36..40] y_offset      (may be absent if leader is short)
/// [40..42] x_padding
/// ```
pub fn parse_leader(buf: &[u8]) -> Result<Leader, U3vError> {
    if buf.len() < MIN_LEADER_SIZE {
        return Err(U3vError::Protocol(format!(
            "leader too short: {} bytes, need at least {MIN_LEADER_SIZE}",
            buf.len()
        )));
    }

    let prefix = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if prefix != LEADER_PREFIX {
        return Err(U3vError::Protocol(format!(
            "bad leader prefix: {prefix:#010x}, expected {LEADER_PREFIX:#010x}"
        )));
    }

    let payload_type = u16::from_le_bytes([buf[10], buf[11]]);
    let timestamp = u64::from_le_bytes(buf[12..20].try_into().unwrap());
    let pixel_format = u32::from_le_bytes(buf[20..24].try_into().unwrap());
    let width = u32::from_le_bytes(buf[24..28].try_into().unwrap());
    let height = u32::from_le_bytes(buf[28..32].try_into().unwrap());
    let x_offset = u32::from_le_bytes(buf[32..36].try_into().unwrap());

    // Optional fields (may be absent in minimal leaders).
    let y_offset = if buf.len() >= 40 {
        u32::from_le_bytes(buf[36..40].try_into().unwrap())
    } else {
        0
    };
    let x_padding = if buf.len() >= 42 {
        u16::from_le_bytes([buf[40], buf[41]])
    } else {
        0
    };

    Ok(Leader {
        payload_type,
        timestamp,
        pixel_format,
        width,
        height,
        x_offset,
        y_offset,
        x_padding,
    })
}

/// Parse a U3V stream trailer from raw bytes.
///
/// Trailer layout (little-endian):
/// ```text
/// [0..4]   prefix              0x54563355 ("U3VT")
/// [4..6]   reserved
/// [6..8]   trailer_size
/// [8..12]  status
/// [12..20] block_id            (u64)
/// [20..28] valid_payload_size  (u64, may be absent)
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

    let status = u32::from_le_bytes(buf[8..12].try_into().unwrap());
    let block_id = if buf.len() >= 20 {
        u64::from_le_bytes(buf[12..20].try_into().unwrap())
    } else {
        0
    };
    let valid_payload_size = if buf.len() >= 28 {
        u64::from_le_bytes(buf[20..28].try_into().unwrap())
    } else {
        0
    };

    Ok(Trailer {
        status,
        block_id,
        valid_payload_size,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usb::MockUsbTransfer;

    fn build_leader(width: u32, height: u32, pixel_format: u32, timestamp: u64) -> Vec<u8> {
        let mut buf = vec![0u8; 42];
        // Prefix
        buf[0..4].copy_from_slice(&LEADER_PREFIX.to_le_bytes());
        // leader_size
        buf[6..8].copy_from_slice(&42u16.to_le_bytes());
        // payload_type = image
        buf[10..12].copy_from_slice(&0x0001u16.to_le_bytes());
        // timestamp
        buf[12..20].copy_from_slice(&timestamp.to_le_bytes());
        // pixel_format
        buf[20..24].copy_from_slice(&pixel_format.to_le_bytes());
        // width
        buf[24..28].copy_from_slice(&width.to_le_bytes());
        // height
        buf[28..32].copy_from_slice(&height.to_le_bytes());
        // x_offset = 0, y_offset = 0, x_padding = 0
        buf
    }

    fn build_trailer(status: u32, block_id: u64, valid_payload_size: u64) -> Vec<u8> {
        let mut buf = vec![0u8; 28];
        buf[0..4].copy_from_slice(&TRAILER_PREFIX.to_le_bytes());
        buf[6..8].copy_from_slice(&28u16.to_le_bytes());
        buf[8..12].copy_from_slice(&status.to_le_bytes());
        buf[12..20].copy_from_slice(&block_id.to_le_bytes());
        buf[20..28].copy_from_slice(&valid_payload_size.to_le_bytes());
        buf
    }

    #[test]
    fn parse_leader_valid() {
        let data = build_leader(640, 480, 0x0108_0001, 12345);
        let leader = parse_leader(&data).unwrap();
        assert_eq!(leader.width, 640);
        assert_eq!(leader.height, 480);
        assert_eq!(leader.pixel_format, 0x0108_0001); // Mono8
        assert_eq!(leader.timestamp, 12345);
        assert_eq!(leader.payload_type, 0x0001);
        assert_eq!(leader.x_offset, 0);
        assert_eq!(leader.y_offset, 0);
        assert_eq!(leader.x_padding, 0);
    }

    #[test]
    fn parse_leader_bad_prefix() {
        let mut data = build_leader(640, 480, 0, 0);
        data[0] = 0xFF;
        assert!(parse_leader(&data).is_err());
    }

    #[test]
    fn parse_leader_too_short() {
        let data = vec![0u8; 10];
        assert!(parse_leader(&data).is_err());
    }

    #[test]
    fn parse_trailer_valid() {
        let data = build_trailer(0, 42, 307200);
        let trailer = parse_trailer(&data).unwrap();
        assert_eq!(trailer.status, 0);
        assert_eq!(trailer.block_id, 42);
        assert_eq!(trailer.valid_payload_size, 307200);
    }

    #[test]
    fn parse_trailer_bad_prefix() {
        let mut data = build_trailer(0, 0, 0);
        data[0] = 0xFF;
        assert!(parse_trailer(&data).is_err());
    }

    #[test]
    fn parse_trailer_too_short() {
        let data = vec![0u8; 8];
        assert!(parse_trailer(&data).is_err());
    }

    #[test]
    fn stream_next_frame() {
        let mock = Arc::new(MockUsbTransfer::new());
        let ep_in = 0x82;

        let width = 320u32;
        let height = 240u32;
        let pixel_format = 0x0108_0001u32; // Mono8
        let payload_size = (width * height) as usize;

        // Enqueue leader
        mock.enqueue_read(ep_in, build_leader(width, height, pixel_format, 99));
        // Enqueue payload
        mock.enqueue_read(ep_in, vec![0x80; payload_size]);
        // Enqueue trailer
        mock.enqueue_read(ep_in, build_trailer(0, 1, payload_size as u64));

        let mut stream = U3vStream::new(Arc::clone(&mock), ep_in, 64, 64, payload_size);
        let frame = stream.next_frame().unwrap();

        assert_eq!(frame.leader.width, width);
        assert_eq!(frame.leader.height, height);
        assert_eq!(frame.leader.pixel_format, pixel_format);
        assert_eq!(frame.leader.timestamp, 99);
        assert_eq!(frame.payload.len(), payload_size);
        assert_eq!(frame.trailer.status, 0);
        assert_eq!(frame.trailer.block_id, 1);
        assert_eq!(frame.trailer.valid_payload_size, payload_size as u64);
    }

    #[test]
    fn stream_multiple_frames() {
        let mock = Arc::new(MockUsbTransfer::new());
        let ep_in = 0x82;

        for i in 0..3u64 {
            mock.enqueue_read(ep_in, build_leader(64, 64, 0x0108_0001, i * 1000));
            mock.enqueue_read(ep_in, vec![0xAA; 64 * 64]);
            mock.enqueue_read(ep_in, build_trailer(0, i, 64 * 64));
        }

        let mut stream = U3vStream::new(Arc::clone(&mock), ep_in, 64, 64, 64 * 64);

        for i in 0..3u64 {
            let frame = stream.next_frame().unwrap();
            assert_eq!(frame.trailer.block_id, i);
            assert_eq!(frame.leader.timestamp, i * 1000);
        }
    }

    /// Payload split across multiple USB bulk transfers should be reassembled.
    #[test]
    fn stream_multi_transfer_payload() {
        let mock = Arc::new(MockUsbTransfer::new());
        let ep_in = 0x82;
        let payload_size = 1024usize;

        mock.enqueue_read(ep_in, build_leader(32, 32, 0x0108_0001, 42));
        // Split payload into 4 chunks of 256 bytes each.
        for chunk in 0..4u8 {
            mock.enqueue_read(ep_in, vec![chunk; 256]);
        }
        mock.enqueue_read(ep_in, build_trailer(0, 1, payload_size as u64));

        let mut stream = U3vStream::new(Arc::clone(&mock), ep_in, 64, 64, payload_size);
        let frame = stream.next_frame().unwrap();

        assert_eq!(frame.payload.len(), payload_size);
        // Verify each 256-byte chunk has the correct fill byte.
        for chunk in 0..4u8 {
            let start = chunk as usize * 256;
            assert!(
                frame.payload[start..start + 256]
                    .iter()
                    .all(|&b| b == chunk),
                "chunk {chunk} has wrong data"
            );
        }
    }
}
