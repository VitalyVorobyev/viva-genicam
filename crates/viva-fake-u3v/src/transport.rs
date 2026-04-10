//! Fake USB transport implementing [`UsbTransfer`] via an in-memory register map.
//!
//! Handles U3V-prefixed GenCP commands (ReadMem, WriteMem) and generates
//! U3V stream frames on the stream endpoint.

use std::sync::Mutex;
use std::time::Duration;

use bytes::{BufMut, BytesMut};
use viva_gencp::{OpCode, StatusCode};
use viva_u3v::usb::UsbTransfer;
use viva_u3v::U3vError;

use crate::registers::RegisterMap;

/// U3V command prefix magic: "U3VC" in little-endian.
const CMD_PREFIX: u32 = 0x4356_3355;
/// U3V acknowledge prefix magic: "U3VA" in little-endian.
const ACK_PREFIX: u32 = 0x4356_3341;
/// U3V prefix size.
const PREFIX_SIZE: usize = 12;

/// U3V stream leader prefix: "U3VL" in little-endian.
const LEADER_PREFIX: u32 = 0x4C56_3355;
/// U3V stream trailer prefix: "U3VT" in little-endian.
const TRAILER_PREFIX: u32 = 0x5456_3355;

/// Fake USB transport backed by an in-memory register map.
///
/// Implements [`UsbTransfer`] by intercepting bulk writes as GenCP commands
/// and returning pre-computed ack responses. Stream endpoint reads return
/// synthetic frames.
pub struct FakeU3vTransport {
    state: Mutex<TransportState>,
}

struct TransportState {
    registers: RegisterMap,
    /// Pending ack response (written by bulk_write, read by bulk_read on ep_in).
    pending_ack: Option<Vec<u8>>,
    /// Frame counter for stream.
    frame_count: u64,
    width: u32,
    height: u32,
    pixel_format: u32,
}

// SAFETY: TransportState is protected by Mutex, single-writer access.
unsafe impl Send for FakeU3vTransport {}
unsafe impl Sync for FakeU3vTransport {}

impl FakeU3vTransport {
    /// Create a new fake transport with the given image dimensions.
    pub fn new(width: u32, height: u32, pixel_format: u32) -> Self {
        Self {
            state: Mutex::new(TransportState {
                registers: RegisterMap::new(width, height, pixel_format),
                pending_ack: None,
                frame_count: 0,
                width,
                height,
                pixel_format,
            }),
        }
    }
}

impl UsbTransfer for FakeU3vTransport {
    fn bulk_write(
        &self,
        _endpoint: u8,
        data: &[u8],
        _timeout: Duration,
    ) -> Result<usize, U3vError> {
        let mut state = self.state.lock().unwrap();

        if data.len() < PREFIX_SIZE {
            return Err(U3vError::Protocol("command too short".into()));
        }

        let prefix = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        if prefix != CMD_PREFIX {
            return Err(U3vError::Protocol(format!("bad cmd prefix: {prefix:#x}")));
        }

        let opcode_raw = u16::from_le_bytes([data[6], data[7]]);
        let scd_len = u16::from_le_bytes([data[8], data[9]]) as usize;
        let request_id = u16::from_le_bytes([data[10], data[11]]);
        let payload = &data[PREFIX_SIZE..PREFIX_SIZE + scd_len];

        let ack = match opcode_raw {
            0x0084 => {
                // ReadMem: payload = [8-byte addr][2-byte reserved][2-byte count]
                let addr = u64::from_be_bytes(payload[0..8].try_into().unwrap());
                let count = u16::from_be_bytes(payload[10..12].try_into().unwrap()) as usize;
                let mem = state.registers.read(addr, count);
                build_ack(StatusCode::Success, OpCode::ReadMem, request_id, &mem)
            }
            0x0086 => {
                // WriteMem: payload = [8-byte addr][data...]
                let addr = u64::from_be_bytes(payload[0..8].try_into().unwrap());
                let write_data = &payload[8..];
                state.registers.write(addr, write_data);
                build_ack(StatusCode::Success, OpCode::WriteMem, request_id, &[])
            }
            0x0080 => {
                // ReadRegister: payload = [8-byte addr]
                let addr = u64::from_be_bytes(payload[0..8].try_into().unwrap());
                let mem = state.registers.read(addr, 4);
                build_ack(StatusCode::Success, OpCode::ReadRegister, request_id, &mem)
            }
            0x0082 => {
                // WriteRegister: payload = [8-byte addr][4-byte value]
                let addr = u64::from_be_bytes(payload[0..8].try_into().unwrap());
                let value = &payload[8..12];
                state.registers.write(addr, value);
                build_ack(StatusCode::Success, OpCode::WriteRegister, request_id, &[])
            }
            _ => build_ack(StatusCode::NotImplemented, OpCode::ReadMem, request_id, &[]),
        };

        state.pending_ack = Some(ack);
        Ok(data.len())
    }

    fn bulk_read(
        &self,
        endpoint: u8,
        buf: &mut [u8],
        _timeout: Duration,
    ) -> Result<usize, U3vError> {
        let mut state = self.state.lock().unwrap();

        // Stream endpoint: generate a frame.
        if endpoint == 0x82 {
            return generate_stream_transfer(&mut state, buf);
        }

        // Control endpoint: return pending ack.
        let ack = state.pending_ack.take().ok_or_else(|| {
            U3vError::Protocol("no pending ack (bulk_read before bulk_write)".into())
        })?;
        let n = ack.len().min(buf.len());
        buf[..n].copy_from_slice(&ack[..n]);
        Ok(n)
    }
}

/// Build a U3V ack packet.
fn build_ack(status: StatusCode, opcode: OpCode, request_id: u16, payload: &[u8]) -> Vec<u8> {
    let mut buf = BytesMut::with_capacity(PREFIX_SIZE + payload.len());
    buf.put_u32_le(ACK_PREFIX);
    buf.put_u16_le(status.to_raw());
    buf.put_u16_le(opcode.ack_code());
    buf.put_u16_le(payload.len() as u16);
    buf.put_u16_le(request_id);
    buf.extend_from_slice(payload);
    buf.to_vec()
}

/// Thread-local stream phase tracker (stored in TransportState via frame_count).
/// We cycle: frame_count % 3 == 0 → leader, 1 → payload, 2 → trailer.
fn generate_stream_transfer(state: &mut TransportState, buf: &mut [u8]) -> Result<usize, U3vError> {
    let phase = state.frame_count % 3;
    state.frame_count += 1;

    match phase {
        0 => {
            // Leader
            let leader = build_leader(state.width, state.height, state.pixel_format);
            let n = leader.len().min(buf.len());
            buf[..n].copy_from_slice(&leader[..n]);
            Ok(n)
        }
        1 => {
            // Payload: synthetic gray ramp.
            let size = (state.width * state.height) as usize;
            let n = size.min(buf.len());
            for (i, byte) in buf.iter_mut().enumerate().take(n) {
                *byte = (i % 256) as u8;
            }
            Ok(n)
        }
        2 => {
            // Trailer
            let block_id = state.frame_count / 3;
            let payload_size = (state.width * state.height) as u64;
            let trailer = build_trailer(block_id, payload_size);
            let n = trailer.len().min(buf.len());
            buf[..n].copy_from_slice(&trailer[..n]);
            Ok(n)
        }
        _ => unreachable!(),
    }
}

fn build_leader(width: u32, height: u32, pixel_format: u32) -> Vec<u8> {
    let mut buf = vec![0u8; 42];
    buf[0..4].copy_from_slice(&LEADER_PREFIX.to_le_bytes());
    buf[6..8].copy_from_slice(&42u16.to_le_bytes());
    buf[10..12].copy_from_slice(&0x0001u16.to_le_bytes()); // payload_type = image
    buf[12..20].copy_from_slice(&0u64.to_le_bytes()); // timestamp
    buf[20..24].copy_from_slice(&pixel_format.to_le_bytes());
    buf[24..28].copy_from_slice(&width.to_le_bytes());
    buf[28..32].copy_from_slice(&height.to_le_bytes());
    buf
}

fn build_trailer(block_id: u64, valid_payload_size: u64) -> Vec<u8> {
    let mut buf = vec![0u8; 28];
    buf[0..4].copy_from_slice(&TRAILER_PREFIX.to_le_bytes());
    buf[6..8].copy_from_slice(&28u16.to_le_bytes());
    // status = 0 (success)
    buf[12..20].copy_from_slice(&block_id.to_le_bytes());
    buf[20..28].copy_from_slice(&valid_payload_size.to_le_bytes());
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use viva_u3v::device::U3vDevice;

    #[test]
    fn fake_transport_open_device() {
        let transport = Arc::new(FakeU3vTransport::new(640, 480, 0x0108_0001));
        let device = U3vDevice::open(transport, 0x81, 0x01, Some(0x82), None).unwrap();
        assert_eq!(device.abrm().manufacturer_name, "FakeCorp");
        assert_eq!(device.abrm().model_name, "FakeU3V");
        assert_eq!(device.sbrm().max_cmd_transfer, 4096);
    }

    #[test]
    fn fake_transport_fetch_xml() {
        let transport = Arc::new(FakeU3vTransport::new(320, 240, 0x0108_0001));
        let mut device = U3vDevice::open(transport, 0x81, 0x01, None, None).unwrap();
        let xml = device.fetch_xml().unwrap();
        assert!(xml.contains("RegisterDescription"));
        assert!(xml.contains("Width"));
        assert!(xml.contains("Height"));
    }

    #[test]
    fn fake_transport_stream_frame() {
        let transport = Arc::new(FakeU3vTransport::new(64, 64, 0x0108_0001));
        let payload_size = 64 * 64;

        let mut stream = viva_u3v::stream::U3vStream::new(transport, 0x82, 256, 256, payload_size);

        let frame = stream.next_frame().unwrap();
        assert_eq!(frame.leader.width, 64);
        assert_eq!(frame.leader.height, 64);
        assert_eq!(frame.payload.len(), payload_size);
    }
}
