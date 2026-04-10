//! GenCP control channel over USB3 Vision bulk endpoints.
//!
//! USB3 Vision wraps GenCP commands/acknowledgements in a 12-byte prefix
//! that carries the same semantic fields (opcode, flags, status, request ID)
//! but in a different wire layout than the standard 8-byte GenCP header.
//!
//! This module encodes/decodes the U3V prefix and delegates to
//! [`viva_gencp`] types for opcodes, status codes, and command flags.

use std::sync::Arc;
use std::time::Duration;

use bytes::{BufMut, BytesMut};
use viva_gencp::{CommandFlags, OpCode, StatusCode};

use crate::U3vError;
use crate::usb::UsbTransfer;

/// U3V command prefix magic: "U3VC" in little-endian.
const CMD_PREFIX: u32 = 0x4356_3355;
/// U3V acknowledge prefix magic: "U3VA" in little-endian (unused for encoding,
/// validated on decode).
const ACK_PREFIX: u32 = 0x4356_3341;

/// Size of the U3V command/ack prefix in bytes.
const PREFIX_SIZE: usize = 12;

/// Status code indicating the device needs more time (PENDING_ACK).
const STATUS_PENDING_ACK: u16 = 0x8006;

/// Maximum number of PENDING_ACK loops before giving up.
const MAX_PENDING_RETRIES: usize = 100;

/// Default control channel timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1000);

/// GenCP control channel over a USB3 Vision bulk endpoint pair.
///
/// Sends GenCP commands (ReadReg, WriteReg, ReadMem, WriteMem) wrapped in
/// the U3V prefix and receives acknowledgements from the device.
pub struct ControlChannel<T: UsbTransfer> {
    transport: Arc<T>,
    ep_in: u8,
    ep_out: u8,
    request_id: u16,
    max_cmd_transfer: u32,
    max_ack_transfer: u32,
    timeout: Duration,
}

impl<T: UsbTransfer> ControlChannel<T> {
    /// Create a new control channel.
    ///
    /// `max_cmd_transfer` and `max_ack_transfer` come from the device's
    /// SBRM (or USB descriptor) and cap the largest single bulk transfer.
    pub fn new(
        transport: Arc<T>,
        ep_in: u8,
        ep_out: u8,
        max_cmd_transfer: u32,
        max_ack_transfer: u32,
    ) -> Self {
        Self {
            transport,
            ep_in,
            ep_out,
            request_id: 0,
            max_cmd_transfer,
            max_ack_transfer,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Override the default control transaction timeout.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Access the underlying transport (e.g. to share with a stream).
    pub fn transport(&self) -> &Arc<T> {
        &self.transport
    }

    /// Read a single 32-bit register at `addr`.
    pub fn read_register(&mut self, addr: u64) -> Result<u32, U3vError> {
        let mut payload = BytesMut::with_capacity(8);
        payload.put_u64(addr);
        let ack = self.transact(OpCode::ReadRegister, &payload)?;
        if ack.len() < 4 {
            return Err(U3vError::Protocol(format!(
                "ReadRegister ack too short: {} bytes",
                ack.len()
            )));
        }
        Ok(u32::from_be_bytes([ack[0], ack[1], ack[2], ack[3]]))
    }

    /// Write a single 32-bit register at `addr`.
    pub fn write_register(&mut self, addr: u64, value: u32) -> Result<(), U3vError> {
        let mut payload = BytesMut::with_capacity(12);
        payload.put_u64(addr);
        payload.put_u32(value);
        let _ack = self.transact(OpCode::WriteRegister, &payload)?;
        Ok(())
    }

    /// Read `len` bytes starting at `addr`, automatically chunking across
    /// the device's maximum acknowledgement transfer size.
    pub fn read_mem(&mut self, addr: u64, len: usize) -> Result<Vec<u8>, U3vError> {
        let max_payload = self.max_read_chunk();
        let mut result = Vec::with_capacity(len);
        let mut offset = 0usize;

        while offset < len {
            let chunk = (len - offset).min(max_payload);
            let mut payload = BytesMut::with_capacity(12);
            payload.put_u64(addr + offset as u64);
            // ReadMem SCD: 8-byte address + 2-byte reserved + 2-byte count
            payload.put_u16(0); // reserved
            payload.put_u16(chunk as u16);
            let ack = self.transact(OpCode::ReadMem, &payload)?;
            result.extend_from_slice(&ack);
            offset += chunk;
        }
        Ok(result)
    }

    /// Write `data` starting at `addr`, automatically chunking across
    /// the device's maximum command transfer size.
    pub fn write_mem(&mut self, addr: u64, data: &[u8]) -> Result<(), U3vError> {
        let max_payload = self.max_write_chunk();
        let mut offset = 0usize;

        while offset < data.len() {
            let chunk = (data.len() - offset).min(max_payload);
            let mut payload = BytesMut::with_capacity(8 + chunk);
            payload.put_u64(addr + offset as u64);
            payload.extend_from_slice(&data[offset..offset + chunk]);
            let _ack = self.transact(OpCode::WriteMem, &payload)?;
            offset += chunk;
        }
        Ok(())
    }

    /// Maximum read chunk size: ack transfer limit minus the prefix.
    fn max_read_chunk(&self) -> usize {
        (self.max_ack_transfer as usize).saturating_sub(PREFIX_SIZE)
    }

    /// Maximum write chunk size: cmd transfer limit minus prefix and 8-byte address.
    fn max_write_chunk(&self) -> usize {
        (self.max_cmd_transfer as usize).saturating_sub(PREFIX_SIZE + 8)
    }

    // -----------------------------------------------------------------------
    // Core transaction: encode → send → receive (with PENDING_ACK retry)
    // -----------------------------------------------------------------------

    fn transact(&mut self, opcode: OpCode, payload: &[u8]) -> Result<Vec<u8>, U3vError> {
        let request_id = self.next_request_id();
        let packet = encode_command(opcode, CommandFlags::ACK_REQUIRED, request_id, payload);
        self.transport
            .bulk_write(self.ep_out, &packet, self.timeout)?;

        // Read ack, handling PENDING_ACK loops.
        let mut ack_buf = vec![0u8; self.max_ack_transfer as usize];
        for _ in 0..MAX_PENDING_RETRIES {
            let n = self
                .transport
                .bulk_read(self.ep_in, &mut ack_buf, self.timeout)?;
            let ack = decode_ack(&ack_buf[..n])?;

            if ack.request_id != request_id {
                return Err(U3vError::Protocol(format!(
                    "request ID mismatch: expected {request_id:#06x}, got {:#06x}",
                    ack.request_id
                )));
            }

            match ack.status {
                StatusCode::Success => return Ok(ack.payload),
                StatusCode::Unknown(STATUS_PENDING_ACK) => {
                    let wait_ms = if ack.payload.len() >= 4 {
                        u32::from_be_bytes([
                            ack.payload[0],
                            ack.payload[1],
                            ack.payload[2],
                            ack.payload[3],
                        ]) as u64
                    } else {
                        100
                    };
                    tracing::debug!(wait_ms, "PENDING_ACK, waiting");
                    std::thread::sleep(Duration::from_millis(wait_ms));
                    continue;
                }
                status => return Err(U3vError::Status { status }),
            }
        }
        Err(U3vError::Timeout)
    }

    fn next_request_id(&mut self) -> u16 {
        let id = self.request_id;
        self.request_id = self.request_id.wrapping_add(1);
        id
    }
}

// ---------------------------------------------------------------------------
// Wire encoding / decoding
// ---------------------------------------------------------------------------

/// Encode a U3V command packet (prefix + GenCP payload).
///
/// Layout (little-endian):
/// ```text
/// [0..4]  prefix   0x43563355 ("U3VC")
/// [4..6]  flags    CommandFlags bits
/// [6..8]  command  OpCode command code
/// [8..10] length   payload length in bytes
/// [10..12] req_id  request identifier
/// [12..]  payload  GenCP-specific payload
/// ```
fn encode_command(opcode: OpCode, flags: CommandFlags, request_id: u16, payload: &[u8]) -> Vec<u8> {
    let mut buf = BytesMut::with_capacity(PREFIX_SIZE + payload.len());
    buf.put_u32_le(CMD_PREFIX);
    buf.put_u16_le(flags.bits());
    buf.put_u16_le(opcode.command_code());
    buf.put_u16_le(payload.len() as u16);
    buf.put_u16_le(request_id);
    buf.extend_from_slice(payload);
    buf.to_vec()
}

/// Decoded U3V acknowledgement fields.
struct AckPacket {
    status: StatusCode,
    request_id: u16,
    payload: Vec<u8>,
}

/// Decode a U3V acknowledgement packet.
fn decode_ack(buf: &[u8]) -> Result<AckPacket, U3vError> {
    if buf.len() < PREFIX_SIZE {
        return Err(U3vError::Protocol(format!(
            "ack too short: {} bytes, need at least {PREFIX_SIZE}",
            buf.len()
        )));
    }

    let prefix = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if prefix != ACK_PREFIX {
        return Err(U3vError::Protocol(format!(
            "bad ack prefix: {prefix:#010x}, expected {ACK_PREFIX:#010x}"
        )));
    }

    let status_raw = u16::from_le_bytes([buf[4], buf[5]]);
    let _opcode = u16::from_le_bytes([buf[6], buf[7]]);
    let length = u16::from_le_bytes([buf[8], buf[9]]) as usize;
    let request_id = u16::from_le_bytes([buf[10], buf[11]]);

    let expected = PREFIX_SIZE + length;
    if buf.len() < expected {
        return Err(U3vError::Protocol(format!(
            "ack truncated: got {} bytes, header says {expected}",
            buf.len()
        )));
    }

    let status = StatusCode::from_raw(status_raw);
    let payload = buf[PREFIX_SIZE..PREFIX_SIZE + length].to_vec();

    Ok(AckPacket {
        status,
        request_id,
        payload,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usb::MockUsbTransfer;

    const EP_OUT: u8 = 0x01;
    const EP_IN: u8 = 0x81;

    /// Build a mock ack response with the given status, request_id, and payload.
    fn build_ack(status: StatusCode, request_id: u16, payload: &[u8]) -> Vec<u8> {
        let mut buf = BytesMut::with_capacity(PREFIX_SIZE + payload.len());
        buf.put_u32_le(ACK_PREFIX);
        buf.put_u16_le(status.to_raw());
        buf.put_u16_le(OpCode::ReadMem.ack_code()); // opcode doesn't matter for most tests
        buf.put_u16_le(payload.len() as u16);
        buf.put_u16_le(request_id);
        buf.extend_from_slice(payload);
        buf.to_vec()
    }

    fn make_channel(mock: &Arc<MockUsbTransfer>) -> ControlChannel<MockUsbTransfer> {
        ControlChannel::new(Arc::clone(mock), EP_IN, EP_OUT, 1024, 1024)
    }

    #[test]
    fn encode_command_format() {
        let payload = [0xAA, 0xBB, 0xCC, 0xDD];
        let pkt = encode_command(
            OpCode::ReadMem,
            CommandFlags::ACK_REQUIRED,
            0x0042,
            &payload,
        );
        assert_eq!(pkt.len(), PREFIX_SIZE + 4);

        // Prefix
        assert_eq!(
            u32::from_le_bytes([pkt[0], pkt[1], pkt[2], pkt[3]]),
            CMD_PREFIX
        );
        // Flags
        assert_eq!(
            u16::from_le_bytes([pkt[4], pkt[5]]),
            CommandFlags::ACK_REQUIRED.bits()
        );
        // Opcode
        assert_eq!(
            u16::from_le_bytes([pkt[6], pkt[7]]),
            OpCode::ReadMem.command_code()
        );
        // Length
        assert_eq!(u16::from_le_bytes([pkt[8], pkt[9]]), 4);
        // Request ID
        assert_eq!(u16::from_le_bytes([pkt[10], pkt[11]]), 0x0042);
        // Payload
        assert_eq!(&pkt[12..], &payload);
    }

    #[test]
    fn decode_ack_success() {
        let payload = vec![0x01, 0x02, 0x03, 0x04];
        let buf = build_ack(StatusCode::Success, 0x0042, &payload);
        let ack = decode_ack(&buf).unwrap();
        assert_eq!(ack.status, StatusCode::Success);
        assert_eq!(ack.request_id, 0x0042);
        assert_eq!(ack.payload, payload);
    }

    #[test]
    fn decode_ack_bad_prefix() {
        let mut buf = build_ack(StatusCode::Success, 0x0001, &[]);
        // Corrupt prefix
        buf[0] = 0xFF;
        assert!(decode_ack(&buf).is_err());
    }

    #[test]
    fn decode_ack_truncated() {
        let buf = build_ack(StatusCode::Success, 0x0001, &[1, 2, 3, 4]);
        // Truncate: send header but chop off payload
        assert!(decode_ack(&buf[..PREFIX_SIZE]).is_err());
    }

    #[test]
    fn read_register_roundtrip() {
        let mock = Arc::new(MockUsbTransfer::new());
        let mut ch = make_channel(&mock);

        // Enqueue a success ack with a 4-byte register value
        let value: u32 = 0xDEAD_BEEF;
        let ack = build_ack(StatusCode::Success, 0x0000, &value.to_be_bytes());
        mock.enqueue_read(EP_IN, ack);

        let result = ch.read_register(0x0000_1000).unwrap();
        assert_eq!(result, value);

        // Verify the command was sent
        let writes = mock.take_writes(EP_OUT);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].len(), PREFIX_SIZE + 8); // prefix + 8-byte addr
    }

    #[test]
    fn write_register_roundtrip() {
        let mock = Arc::new(MockUsbTransfer::new());
        let mut ch = make_channel(&mock);

        // Enqueue success ack (empty payload is fine for write)
        let ack = build_ack(StatusCode::Success, 0x0000, &[]);
        mock.enqueue_read(EP_IN, ack);

        ch.write_register(0x0000_1000, 0x1234_5678).unwrap();

        let writes = mock.take_writes(EP_OUT);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].len(), PREFIX_SIZE + 12); // prefix + 8-byte addr + 4-byte value
    }

    #[test]
    fn read_mem_single_chunk() {
        let mock = Arc::new(MockUsbTransfer::new());
        let mut ch = make_channel(&mock);

        let data = vec![0xAA; 64];
        let ack = build_ack(StatusCode::Success, 0x0000, &data);
        mock.enqueue_read(EP_IN, ack);

        let result = ch.read_mem(0x0000_2000, 64).unwrap();
        assert_eq!(result, data);
    }

    #[test]
    fn read_mem_chunked() {
        // Set max_ack_transfer small enough to force 2 chunks for 64 bytes.
        // max_read_chunk = max_ack_transfer - PREFIX_SIZE = 48 - 12 = 36
        let mock = Arc::new(MockUsbTransfer::new());
        let mut ch = ControlChannel::new(Arc::clone(&mock), EP_IN, EP_OUT, 1024, 48);

        let chunk1 = vec![0xAA; 36];
        let chunk2 = vec![0xBB; 28];
        mock.enqueue_read(EP_IN, build_ack(StatusCode::Success, 0x0000, &chunk1));
        mock.enqueue_read(EP_IN, build_ack(StatusCode::Success, 0x0001, &chunk2));

        let result = ch.read_mem(0x0000_3000, 64).unwrap();
        assert_eq!(result.len(), 64);
        assert_eq!(&result[..36], &chunk1[..]);
        assert_eq!(&result[36..], &chunk2[..]);

        // Should have issued 2 write commands
        let writes = mock.take_writes(EP_OUT);
        assert_eq!(writes.len(), 2);
    }

    #[test]
    fn write_mem_chunked() {
        let mock = Arc::new(MockUsbTransfer::new());
        // max_write_chunk = 48 - 12 - 8 = 28
        let mut ch = ControlChannel::new(Arc::clone(&mock), EP_IN, EP_OUT, 48, 1024);

        mock.enqueue_read(EP_IN, build_ack(StatusCode::Success, 0x0000, &[]));
        mock.enqueue_read(EP_IN, build_ack(StatusCode::Success, 0x0001, &[]));

        let data = vec![0xCC; 50]; // > 28, so needs 2 chunks
        ch.write_mem(0x0000_4000, &data).unwrap();

        let writes = mock.take_writes(EP_OUT);
        assert_eq!(writes.len(), 2);
    }

    #[test]
    fn pending_ack_retry() {
        let mock = Arc::new(MockUsbTransfer::new());
        let mut ch = make_channel(&mock);

        // First ack: PENDING_ACK with 0ms wait
        let pending_payload = 0u32.to_be_bytes();
        mock.enqueue_read(
            EP_IN,
            build_ack(
                StatusCode::Unknown(STATUS_PENDING_ACK),
                0x0000,
                &pending_payload,
            ),
        );
        // Second ack: success
        let value = 0x1234_5678u32;
        mock.enqueue_read(
            EP_IN,
            build_ack(StatusCode::Success, 0x0000, &value.to_be_bytes()),
        );

        let result = ch.read_register(0x0000_5000).unwrap();
        assert_eq!(result, value);
    }

    #[test]
    fn device_error_status() {
        let mock = Arc::new(MockUsbTransfer::new());
        let mut ch = make_channel(&mock);

        mock.enqueue_read(EP_IN, build_ack(StatusCode::InvalidAddress, 0x0000, &[]));

        let err = ch.read_register(0xFFFF_FFFF).unwrap_err();
        assert!(matches!(
            err,
            U3vError::Status {
                status: StatusCode::InvalidAddress
            }
        ));
    }

    #[test]
    fn request_id_increments() {
        let mock = Arc::new(MockUsbTransfer::new());
        let mut ch = make_channel(&mock);

        // Two reads, request ID should increment
        mock.enqueue_read(EP_IN, build_ack(StatusCode::Success, 0x0000, &[0; 4]));
        mock.enqueue_read(EP_IN, build_ack(StatusCode::Success, 0x0001, &[0; 4]));

        ch.read_register(0x1000).unwrap();
        ch.read_register(0x2000).unwrap();

        let writes = mock.take_writes(EP_OUT);
        assert_eq!(writes.len(), 2);
        // First command: request_id = 0
        assert_eq!(u16::from_le_bytes([writes[0][10], writes[0][11]]), 0x0000);
        // Second command: request_id = 1
        assert_eq!(u16::from_le_bytes([writes[1][10], writes[1][11]]), 0x0001);
    }
}
