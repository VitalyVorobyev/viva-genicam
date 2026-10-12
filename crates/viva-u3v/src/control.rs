//! GenCP control channel over USB3 Vision bulk endpoints.
//!
//! A command and its acknowledge each start with the `U3VC` prefix, then
//! GenCP's common command data (GenCP 1.2, Tables 4-5), then the command's
//! own data (Tables 8-12):
//!
//! ```text
//! command      [ "U3VC" ][ flags ][ command_id ][ length ][ request_id ][ SCD ]
//! acknowledge  [ "U3VC" ][ status ][ command_id ][ length ][ request_id ][ SCD ]
//! ```
//!
//! Every field is little-endian. GenCP leaves the byte order to the
//! technology but requires it to match the bootstrap registers' (§2.3), and
//! for USB3 Vision both are little-endian: aravis (`arvuvcp.c`) and
//! Wireshark (`packet-u3v.c`) encode and decode every field so, and a
//! Grasshopper3's own XML declares its bootstrap registers `LittleEndian`
//! (`PGR_GS3_U3_23S6M.xml`). Until backlog TC-27 this module sent GVCP's
//! opcodes and flag bit with a big-endian address, which no conformant device
//! answers.
//!
//! GenCP has no register commands; a register is a 4-byte memory access.

use std::sync::Arc;
use std::time::Duration;

use bytes::{BufMut, BytesMut};
use viva_gencp::{ccd_flags, command_id};

use crate::U3vStatus;

use crate::U3vError;
use crate::usb::UsbTransfer;

/// `U3VC` read as a little-endian `u32`: the prefix of every control-channel
/// command and acknowledge. aravis `ARV_UVCP_MAGIC`; Wireshark
/// `U3V_CONTROL_PREFIX`.
const CONTROL_PREFIX: u32 = 0x4356_3355;

/// Prefix plus common command data, in bytes.
const HEADER_SIZE: usize = 12;

/// A READMEM_CMD's SCD: address, reserved, read length (GenCP 1.2 Table 8).
const READMEM_SCD_SIZE: usize = 12;

/// The address in front of a WRITEMEM_CMD's data (GenCP 1.2 Table 10).
const WRITEMEM_ADDRESS_SIZE: usize = 8;

/// Maximum number of acknowledges read for one command before giving up:
/// pending acknowledges and stale answers to earlier commands.
const MAX_ACKS_PER_COMMAND: usize = 100;

/// Upper bound on a single pending-acknowledge wait.
///
/// A device asking for more time should not be able to park a control
/// transaction indefinitely, and `MAX_ACKS_PER_COMMAND` alone does not bound
/// the total wait.
const MAX_PENDING_WAIT: Duration = Duration::from_millis(10_000);

/// Wait used when a pending-acknowledge carries no usable timeout field.
const DEFAULT_PENDING_WAIT_MS: u64 = 100;

/// Extract the requested wait from a pending-acknowledge SCD.
///
/// GenCP 1.2 Table 12: 2 reserved bytes followed by a little-endian `u16` of
/// milliseconds. aravis reads the same field the same way
/// (`ArvUvcpPendingAckInfos`, `GUINT16_FROM_LE`).
///
/// The previous reading took all four bytes as a **big-endian `u32`**, which
/// is wrong in both offset and byte order: a device asking for 1 000 ms sends
/// `00 00 E8 03`, which that reading turned into 59 395 ms. See backlog TC-12
/// for the GVCP side of the same field, which is still unsettled against
/// hardware.
fn pending_ack_timeout_ms(scd: &[u8]) -> u64 {
    if scd.len() < 4 {
        return DEFAULT_PENDING_WAIT_MS;
    }
    let requested = u16::from_le_bytes([scd[2], scd[3]]) as u64;
    if requested == 0 {
        return DEFAULT_PENDING_WAIT_MS;
    }
    requested.min(MAX_PENDING_WAIT.as_millis() as u64)
}

/// Default control channel timeout, before the device's Maximum Device
/// Response Time is known.
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1000);

/// GenCP control channel over a USB3 Vision bulk endpoint pair.
///
/// Sends READMEM and WRITEMEM commands and receives their acknowledges,
/// splitting a large access across the device's maximum transfer sizes.
pub struct ControlChannel<T: UsbTransfer> {
    transport: Arc<T>,
    ep_in: u8,
    ep_out: u8,
    /// The request id of the last command sent; 0 before the first.
    request_id: u16,
    max_cmd_transfer: u32,
    max_ack_transfer: u32,
    timeout: Duration,
}

impl<T: UsbTransfer> ControlChannel<T> {
    /// Create a new control channel.
    ///
    /// `max_cmd_transfer` and `max_ack_transfer` cap the largest single bulk
    /// transfer each way; the SBRM has the device's values, which
    /// [`set_max_transfer`](Self::set_max_transfer) applies once read.
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

    /// Use the device's transfer limits from now on, keeping the request id
    /// sequence: a device answers a repeated id with its previous acknowledge
    /// (GenCP §3.1), so the sequence must not restart mid-session.
    pub fn set_max_transfer(&mut self, max_cmd_transfer: u32, max_ack_transfer: u32) {
        self.max_cmd_transfer = max_cmd_transfer;
        self.max_ack_transfer = max_ack_transfer;
    }

    /// Override the control transaction timeout.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// The control transaction timeout.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Access the underlying transport (e.g. to share with a stream).
    pub fn transport(&self) -> &Arc<T> {
        &self.transport
    }

    /// Read `len` bytes starting at `addr`, split across the device's maximum
    /// acknowledge transfer size.
    pub fn read_mem(&mut self, addr: u64, len: usize) -> Result<Vec<u8>, U3vError> {
        let max_payload = self.max_read_chunk()?;
        let mut result = Vec::with_capacity(len);
        let mut offset = 0usize;

        while offset < len {
            let chunk = (len - offset).min(max_payload);
            let mut scd = BytesMut::with_capacity(READMEM_SCD_SIZE);
            scd.put_u64_le(addr + offset as u64);
            scd.put_u16_le(0); // reserved
            scd.put_u16_le(chunk as u16);
            let ack = self.transact(command_id::READMEM_CMD, command_id::READMEM_ACK, &scd)?;
            if ack.len() != chunk {
                return Err(U3vError::Length {
                    what: "READMEM_ACK payload",
                    expected: chunk,
                    got: ack.len(),
                });
            }
            result.extend_from_slice(&ack);
            offset += chunk;
        }
        Ok(result)
    }

    /// Write `data` starting at `addr`, split across the device's maximum
    /// command transfer size. The bytes go to the device as given: register
    /// contents are in the device's own byte order, not the protocol's.
    pub fn write_mem(&mut self, addr: u64, data: &[u8]) -> Result<(), U3vError> {
        let max_payload = self.max_write_chunk()?;
        let mut offset = 0usize;

        while offset < data.len() {
            let chunk = (data.len() - offset).min(max_payload);
            let mut scd = BytesMut::with_capacity(WRITEMEM_ADDRESS_SIZE + chunk);
            scd.put_u64_le(addr + offset as u64);
            scd.extend_from_slice(&data[offset..offset + chunk]);
            let ack = self.transact(command_id::WRITEMEM_CMD, command_id::WRITEMEM_ACK, &scd)?;
            // GenCP 1.2 Table 11: a device with the Written Length Field
            // capability answers with reserved + length_written; one without
            // sends no SCD at all.
            if ack.len() >= 4 {
                let written = u16::from_le_bytes([ack[2], ack[3]]) as usize;
                if written != chunk {
                    return Err(U3vError::Length {
                        what: "a WRITEMEM_ACK's bytes written",
                        expected: chunk,
                        got: written,
                    });
                }
            }
            offset += chunk;
        }
        Ok(())
    }

    /// Maximum read chunk size: the acknowledge transfer limit minus the
    /// header, and no more than the `u16` read length can say.
    fn max_read_chunk(&self) -> Result<usize, U3vError> {
        let chunk = (self.max_ack_transfer as usize)
            .saturating_sub(HEADER_SIZE)
            .min(u16::MAX as usize);
        if chunk == 0 {
            return Err(U3vError::Device(
                "has a maximum acknowledge transfer too small to carry data",
            ));
        }
        Ok(chunk)
    }

    /// Maximum write chunk size: the command transfer limit minus the header
    /// and the address, and no more than the `u16` length field can say.
    fn max_write_chunk(&self) -> Result<usize, U3vError> {
        let chunk = (self.max_cmd_transfer as usize)
            .saturating_sub(HEADER_SIZE + WRITEMEM_ADDRESS_SIZE)
            .min(u16::MAX as usize - WRITEMEM_ADDRESS_SIZE);
        if chunk == 0 {
            return Err(U3vError::Device(
                "has a maximum command transfer too small to carry data",
            ));
        }
        Ok(chunk)
    }

    // -----------------------------------------------------------------------
    // Core transaction: encode → send → receive (with PENDING_ACK handling)
    // -----------------------------------------------------------------------

    fn transact(
        &mut self,
        command: u16,
        expected_ack: u16,
        scd: &[u8],
    ) -> Result<Vec<u8>, U3vError> {
        let request_id = self.next_request_id();
        let packet = encode_command(command, request_id, scd);
        self.transport
            .bulk_write(self.ep_out, &packet, self.timeout)?;

        let mut ack_buf = vec![0u8; (self.max_ack_transfer as usize).max(HEADER_SIZE)];
        let mut wait = self.timeout;
        for _ in 0..MAX_ACKS_PER_COMMAND {
            let n = self.transport.bulk_read(self.ep_in, &mut ack_buf, wait)?;
            let ack = decode_ack(&ack_buf[..n])?;

            // An answer to an earlier command — one that timed out here and
            // was answered late — is not this command's answer.
            if ack.request_id != request_id {
                tracing::debug!(
                    expected = request_id,
                    got = ack.request_id,
                    "discarding an acknowledge for another request"
                );
                continue;
            }

            // A pending-acknowledge is identified by its command id, before
            // the status is consulted at all: its status is `SUCCESS`, so
            // status-first dispatch would accept it as the real answer and
            // return its timeout SCD as register data. aravis orders the two
            // checks the same way (`arvuvdevice.c`). The time it asks for is
            // a new deadline for this command, counted from now (GenCP 1.2
            // §4.4.5).
            if ack.command == command_id::PENDING_ACK {
                let wait_ms = pending_ack_timeout_ms(&ack.payload);
                tracing::debug!(wait_ms, "PENDING_ACK, extending deadline");
                wait = Duration::from_millis(wait_ms) + self.timeout;
                continue;
            }

            // Anything else must be the acknowledgement for the command we
            // sent. Without this the request id alone was the only guard, so
            // any other ack that happened to carry a matching id was accepted.
            if ack.command != expected_ack {
                tracing::debug!(
                    command = format_args!("{:#06x}", ack.command),
                    expected = format_args!("{expected_ack:#06x}"),
                    "acknowledge answers another command"
                );
                return Err(U3vError::Malformed {
                    packet: "acknowledge",
                    problem: "answers another command",
                });
            }

            return match ack.status {
                status if status.is_success() => Ok(ack.payload),
                status => Err(U3vError::Status(status)),
            };
        }
        Err(U3vError::Timeout)
    }

    /// The next request id: never 0, which a device refuses on a write and
    /// executes uncached on a read (GenCP §3.1), so the sequence wraps to 1.
    fn next_request_id(&mut self) -> u16 {
        self.request_id = match self.request_id {
            u16::MAX => 1,
            id => id + 1,
        };
        self.request_id
    }
}

// ---------------------------------------------------------------------------
// Wire encoding / decoding
// ---------------------------------------------------------------------------

/// Encode a command with RequestAck set: a device ignores a command without
/// it and sends nothing back.
fn encode_command(command: u16, request_id: u16, scd: &[u8]) -> Vec<u8> {
    let mut buf = BytesMut::with_capacity(HEADER_SIZE + scd.len());
    buf.put_u32_le(CONTROL_PREFIX);
    buf.put_u16_le(ccd_flags::REQUEST_ACK);
    buf.put_u16_le(command);
    buf.put_u16_le(scd.len() as u16);
    buf.put_u16_le(request_id);
    buf.extend_from_slice(scd);
    buf.to_vec()
}

/// Decoded U3V acknowledgement fields.
struct AckPacket {
    status: U3vStatus,
    /// The acknowledgement's command id.
    ///
    /// Must be kept: a pending-acknowledge is distinguished from a real answer
    /// by this field and not by `status`, which is `SUCCESS` on both.
    command: u16,
    request_id: u16,
    payload: Vec<u8>,
}

/// Decode an acknowledge.
fn decode_ack(buf: &[u8]) -> Result<AckPacket, U3vError> {
    if buf.len() < HEADER_SIZE {
        return Err(U3vError::Malformed {
            packet: "acknowledge",
            problem: "is shorter than its header",
        });
    }

    let prefix = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if prefix != CONTROL_PREFIX {
        return Err(U3vError::Malformed {
            packet: "acknowledge",
            problem: "lacks the U3VC prefix",
        });
    }

    let status_raw = u16::from_le_bytes([buf[4], buf[5]]);
    let command = u16::from_le_bytes([buf[6], buf[7]]);
    let length = u16::from_le_bytes([buf[8], buf[9]]) as usize;
    let request_id = u16::from_le_bytes([buf[10], buf[11]]);

    let expected = HEADER_SIZE + length;
    if buf.len() < expected {
        return Err(U3vError::Malformed {
            packet: "acknowledge",
            problem: "is shorter than its header says",
        });
    }

    Ok(AckPacket {
        status: U3vStatus::from_raw(status_raw),
        command,
        request_id,
        payload: buf[HEADER_SIZE..expected].to_vec(),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usb::MockUsbTransfer;
    use viva_gencp::StatusCode;

    const EP_OUT: u8 = 0x01;
    const EP_IN: u8 = 0x81;

    /// Build an acknowledgement with an explicit command id.
    ///
    /// Every caller states the command it is impersonating: a fixture that
    /// hardcoded one id once hid that the command id was never checked.
    fn build_ack(command: u16, status: StatusCode, request_id: u16, payload: &[u8]) -> Vec<u8> {
        let mut buf = BytesMut::with_capacity(HEADER_SIZE + payload.len());
        buf.put_u32_le(CONTROL_PREFIX);
        buf.put_u16_le(status.to_raw());
        buf.put_u16_le(command);
        buf.put_u16_le(payload.len() as u16);
        buf.put_u16_le(request_id);
        buf.extend_from_slice(payload);
        buf.to_vec()
    }

    fn make_channel(mock: &Arc<MockUsbTransfer>) -> ControlChannel<MockUsbTransfer> {
        ControlChannel::new(Arc::clone(mock), EP_IN, EP_OUT, 1024, 1024)
    }

    #[test]
    fn decode_ack_success() {
        let payload = vec![0x01, 0x02, 0x03, 0x04];
        let buf = build_ack(
            command_id::READMEM_ACK,
            StatusCode::Success,
            0x0042,
            &payload,
        );
        let ack = decode_ack(&buf).unwrap();
        assert!(ack.status.is_success());
        assert_eq!(ack.request_id, 0x0042);
        assert_eq!(ack.payload, payload);
    }

    /// An acknowledge carries the same `U3VC` prefix as a command; the
    /// prefix this module once expected spelled `A3VC` on the wire.
    #[test]
    fn decode_ack_bad_prefix() {
        let mut buf = build_ack(command_id::READMEM_ACK, StatusCode::Success, 0x0001, &[]);
        assert_eq!(&buf[..4], b"U3VC");
        buf[0] = b'A';
        assert!(decode_ack(&buf).is_err());
    }

    #[test]
    fn decode_ack_truncated() {
        let buf = build_ack(
            command_id::READMEM_ACK,
            StatusCode::Success,
            0x0001,
            &[1, 2, 3, 4],
        );
        assert!(decode_ack(&buf[..HEADER_SIZE]).is_err());
    }

    #[test]
    fn read_mem_single_chunk() {
        let mock = Arc::new(MockUsbTransfer::new());
        let mut ch = make_channel(&mock);

        let data = vec![0xAA; 64];
        mock.enqueue_read(
            EP_IN,
            build_ack(command_id::READMEM_ACK, StatusCode::Success, 1, &data),
        );

        assert_eq!(ch.read_mem(0x0000_2000, 64).unwrap(), data);
    }

    #[test]
    fn read_mem_chunked() {
        // max_read_chunk = max_ack_transfer - HEADER_SIZE = 48 - 12 = 36
        let mock = Arc::new(MockUsbTransfer::new());
        let mut ch = ControlChannel::new(Arc::clone(&mock), EP_IN, EP_OUT, 1024, 48);

        let chunk1 = vec![0xAA; 36];
        let chunk2 = vec![0xBB; 28];
        mock.enqueue_read(
            EP_IN,
            build_ack(command_id::READMEM_ACK, StatusCode::Success, 1, &chunk1),
        );
        mock.enqueue_read(
            EP_IN,
            build_ack(command_id::READMEM_ACK, StatusCode::Success, 2, &chunk2),
        );

        let result = ch.read_mem(0x0000_3000, 64).unwrap();
        assert_eq!(&result[..36], &chunk1[..]);
        assert_eq!(&result[36..], &chunk2[..]);

        let writes = mock.take_writes(EP_OUT);
        assert_eq!(writes.len(), 2);
        // The second command reads on from where the first stopped.
        assert_eq!(&writes[1][12..20], &(0x3000u64 + 36).to_le_bytes());
    }

    #[test]
    fn write_mem_chunked() {
        let mock = Arc::new(MockUsbTransfer::new());
        // max_write_chunk = 48 - 12 - 8 = 28
        let mut ch = ControlChannel::new(Arc::clone(&mock), EP_IN, EP_OUT, 48, 1024);

        mock.enqueue_read(
            EP_IN,
            build_ack(command_id::WRITEMEM_ACK, StatusCode::Success, 1, &[]),
        );
        mock.enqueue_read(
            EP_IN,
            build_ack(command_id::WRITEMEM_ACK, StatusCode::Success, 2, &[]),
        );

        ch.write_mem(0x0000_4000, &[0xCC; 50]).unwrap();
        assert_eq!(mock.take_writes(EP_OUT).len(), 2);
    }

    /// A device with the Written Length Field capability says how much it
    /// wrote (GenCP 1.2 Table 11); a short write is an error, not a success.
    #[test]
    fn a_short_write_is_reported() {
        let mock = Arc::new(MockUsbTransfer::new());
        let mut ch = make_channel(&mock);
        mock.enqueue_read(
            EP_IN,
            build_ack(
                command_id::WRITEMEM_ACK,
                StatusCode::Success,
                1,
                &[0, 0, 2, 0],
            ),
        );

        let err = ch.write_mem(0x1000, &[1, 2, 3, 4]).unwrap_err();
        assert!(
            matches!(
                err,
                U3vError::Length {
                    expected: 4,
                    got: 2,
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn pending_ack_retry() {
        let mock = Arc::new(MockUsbTransfer::new());
        let mut ch = make_channel(&mock);

        // First ack: a real GenCP pending-acknowledge. Note the status is
        // SUCCESS -- it is the command id that marks it -- and the SCD is
        // 2 reserved bytes plus a little-endian u16 of milliseconds.
        mock.enqueue_read(
            EP_IN,
            build_ack(command_id::PENDING_ACK, StatusCode::Success, 1, &[0; 4]),
        );
        let value = 0x1234_5678u32.to_le_bytes();
        mock.enqueue_read(
            EP_IN,
            build_ack(command_id::READMEM_ACK, StatusCode::Success, 1, &value),
        );

        assert_eq!(ch.read_mem(0x0000_5000, 4).unwrap(), value);
    }

    #[test]
    fn device_error_status() {
        let mock = Arc::new(MockUsbTransfer::new());
        let mut ch = make_channel(&mock);

        mock.enqueue_read(
            EP_IN,
            build_ack(command_id::READMEM_ACK, StatusCode::InvalidAddress, 1, &[]),
        );

        let err = ch.read_mem(0xFFFF_FFFF, 4).unwrap_err();
        assert!(matches!(
            err,
            U3vError::Status(U3vStatus::Common(StatusCode::InvalidAddress))
        ));
    }

    #[test]
    fn pending_ack_scd_is_a_little_endian_u16_after_two_reserved_bytes() {
        // A device asking for 1000 ms sends 00 00 E8 03.
        assert_eq!(pending_ack_timeout_ms(&[0x00, 0x00, 0xE8, 0x03]), 1000);
        // The previous reading took all four bytes as a big-endian u32, which
        // turned that same request into a 59-second sleep.
        assert_ne!(
            pending_ack_timeout_ms(&[0x00, 0x00, 0xE8, 0x03]),
            u32::from_be_bytes([0x00, 0x00, 0xE8, 0x03]) as u64
        );
        // Too short, and zero, fall back rather than busy-looping.
        assert_eq!(pending_ack_timeout_ms(&[0x00]), DEFAULT_PENDING_WAIT_MS);
        assert_eq!(
            pending_ack_timeout_ms(&[0x00, 0x00, 0x00, 0x00]),
            DEFAULT_PENDING_WAIT_MS
        );
        // A device cannot park the transaction indefinitely.
        assert_eq!(
            pending_ack_timeout_ms(&[0x00, 0x00, 0xFF, 0xFF]),
            MAX_PENDING_WAIT.as_millis() as u64
        );
    }

    #[test]
    fn access_denied_is_reported_not_retried_as_pending() {
        // 0x8006 used to be read as "device needs more time", so a refusal was
        // slept over 100 times and then reported as a timeout.
        let mock = Arc::new(MockUsbTransfer::new());
        let mut ch = make_channel(&mock);

        mock.enqueue_read(
            EP_IN,
            build_ack(command_id::READMEM_ACK, StatusCode::AccessDenied, 1, &[]),
        );

        let err = ch.read_mem(0x0000_1000, 4).unwrap_err();
        assert!(matches!(
            err,
            U3vError::Status(U3vStatus::Common(StatusCode::AccessDenied))
        ));
        // One exchange, not 100.
        assert_eq!(mock.take_writes(EP_OUT).len(), 1);
    }

    #[test]
    fn acknowledgement_for_another_command_is_rejected() {
        let mock = Arc::new(MockUsbTransfer::new());
        let mut ch = make_channel(&mock);

        mock.enqueue_read(
            EP_IN,
            build_ack(
                command_id::WRITEMEM_ACK,
                StatusCode::Success,
                1,
                &0xDEAD_BEEFu32.to_le_bytes(),
            ),
        );

        let err = ch.read_mem(0x0000_1000, 4).unwrap_err();
        assert!(
            matches!(
                err,
                U3vError::Malformed {
                    problem: "answers another command",
                    ..
                }
            ),
            "expected an acknowledge for another command, got {err:?}"
        );
    }

    /// A late answer to a command that already timed out here is skipped,
    /// not taken for this command's answer.
    #[test]
    fn an_acknowledge_for_an_earlier_request_is_skipped() {
        let mock = Arc::new(MockUsbTransfer::new());
        let mut ch = make_channel(&mock);
        mock.enqueue_read(
            EP_IN,
            build_ack(command_id::READMEM_ACK, StatusCode::Success, 7, &[9; 4]),
        );
        mock.enqueue_read(
            EP_IN,
            build_ack(command_id::READMEM_ACK, StatusCode::Success, 1, &[1; 4]),
        );

        assert_eq!(ch.read_mem(0x1000, 4).unwrap(), [1; 4]);
    }

    #[test]
    fn request_ids_start_at_one_and_wrap_past_zero() {
        let mock = Arc::new(MockUsbTransfer::new());
        let mut ch = make_channel(&mock);
        assert_eq!(ch.next_request_id(), 1);
        assert_eq!(ch.next_request_id(), 2);
        ch.request_id = u16::MAX - 1;
        assert_eq!(ch.next_request_id(), u16::MAX);
        // GenCP §3.1: wrap to 1, so id 0 is never reused.
        assert_eq!(ch.next_request_id(), 1);
    }
}
