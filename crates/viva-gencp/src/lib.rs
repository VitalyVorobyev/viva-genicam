#![cfg_attr(docsrs, feature(doc_cfg))]
//! GenCP: generic control protocol encode/decode (transport-agnostic).

use bitflags::bitflags;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use thiserror::Error;

/// Size of the GenCP header (in bytes).
pub const HEADER_SIZE: usize = 8;

bitflags! {
    /// Flags that can be set on a GenCP command packet.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct CommandFlags: u16 {
        /// Request an acknowledgement for this command.
        const ACK_REQUIRED = 0x0001;
        /// Mark the command as a broadcast.
        const BROADCAST = 0x8000;
        /// GVCP `ACTION_CMD` only: the payload carries a scheduled action time.
        ///
        /// These bit values are this crate's own representation; the mapping to
        /// the single GVCP flags byte lives in `viva_gige::gvcp`.
        const SCHEDULED_ACTION = 0x0002;
    }
}

/// GenCP command ids (GenCP 1.2, Table 7) — what USB3 Vision carries.
///
/// These are not [`OpCode`]'s values: those are GVCP's opcodes, which GigE
/// Vision uses for the same operations and which no GenCP device understands.
/// A command id has its least significant bit clear; the acknowledge's is the
/// same id with that bit set.
pub mod command_id {
    /// Read a block of the device's register map.
    pub const READMEM_CMD: u16 = 0x0800;
    /// Answer to [`READMEM_CMD`], carrying the data read.
    pub const READMEM_ACK: u16 = 0x0801;
    /// Write a block of the device's register map.
    pub const WRITEMEM_CMD: u16 = 0x0802;
    /// Answer to [`WRITEMEM_CMD`].
    pub const WRITEMEM_ACK: u16 = 0x0803;
    /// The device needs more time; see [`PENDING_ACK_COMMAND`](super::PENDING_ACK_COMMAND).
    pub const PENDING_ACK: u16 = 0x0805;
    /// An event the device sends unasked.
    pub const EVENT_CMD: u16 = 0x0C00;
    /// The host's answer to [`EVENT_CMD`].
    pub const EVENT_ACK: u16 = 0x0C01;
}

/// Flag bits of a GenCP command's `flags` field (GenCP 1.2, Table 4).
///
/// Not [`CommandFlags`], whose bit values are this crate's GVCP
/// representation: a GenCP device ignores a command without
/// [`REQUEST_ACK`](ccd_flags::REQUEST_ACK) set, and answers nothing.
pub mod ccd_flags {
    /// The sender wants an acknowledge.
    pub const REQUEST_ACK: u16 = 1 << 14;
    /// The command is a retry of one whose acknowledge never arrived.
    pub const COMMAND_RESEND: u16 = 1 << 15;
}

/// Command id of the GenCP pending-acknowledge.
///
/// A device that cannot answer within the controller's timeout replies with
/// this instead of the real acknowledgement, and puts the extra time it wants
/// in the SCD. It is a **command id, not a status** — the status field of a
/// pending-ack is `SUCCESS`, so a receiver that only inspects the status
/// cannot tell one from a real answer and will hand the timeout bytes back as
/// payload. GVCP models the same mechanism the same way, as opcode `0x0089`
/// (`viva_gige::gvcp::consts::PENDING_ACK`).
pub const PENDING_ACK_COMMAND: u16 = command_id::PENDING_ACK;

/// GVCP operation codes for the GenCP-style operations GigE Vision carries.
///
/// The values are GVCP's (`READREG_CMD` `0x0080` …). A USB3 Vision device
/// speaks GenCP proper and knows none of them: its ids are in [`command_id`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpCode {
    /// Read a single bootstrap or device register.
    ReadRegister,
    /// Write a single bootstrap or device register.
    WriteRegister,
    /// Read a block of memory from the device.
    ReadMem,
    /// Write a block of memory to the device.
    WriteMem,
}

impl OpCode {
    /// Raw command value as GVCP defines it.
    pub const fn command_code(self) -> u16 {
        match self {
            OpCode::ReadRegister => 0x0080,
            OpCode::WriteRegister => 0x0082,
            OpCode::ReadMem => 0x0084,
            OpCode::WriteMem => 0x0086,
        }
    }

    /// Raw acknowledgement value as defined by the specification.
    pub const fn ack_code(self) -> u16 {
        self.command_code() + 1
    }

    #[allow(dead_code)]
    fn from_command(code: u16) -> Result<Self, GenCpError> {
        match code {
            0x0080 => Ok(OpCode::ReadRegister),
            0x0082 => Ok(OpCode::WriteRegister),
            0x0084 => Ok(OpCode::ReadMem),
            0x0086 => Ok(OpCode::WriteMem),
            _ => Err(GenCpError::UnknownOpcode(code)),
        }
    }

    fn from_ack(code: u16) -> Result<Self, GenCpError> {
        match code {
            0x0081 => Ok(OpCode::ReadRegister),
            0x0083 => Ok(OpCode::WriteRegister),
            0x0085 => Ok(OpCode::ReadMem),
            0x0087 => Ok(OpCode::WriteMem),
            _ => Err(GenCpError::UnknownOpcode(code)),
        }
    }
}

/// Status codes shared by the GVCP and GenCP acknowledgement tables.
///
/// Only the codes **both** protocols define identically live here:
/// `0x0000`, `0x8001`–`0x8007` and `0x8FFF`. Above that the tables diverge,
/// and at `0x800B` they actively disagree — GVCP calls it `NO_MSG`
/// (deprecated), GenCP calls it `MSG_TIMEOUT` — so a transport-specific code
/// must be decoded by that transport, not here. See ADR-0020.
///
/// Values corroborated by Wireshark's GVCP dissector
/// (`epan/dissectors/packet-gvcp.c`, `GEV_STATUS_*`) and aravis
/// (`arvgvcpprivate.h`, `arvuvcpprivate.h`), which agree with each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusCode {
    /// Command completed successfully.
    Success,
    /// The requested command is not implemented by the device.
    NotImplemented,
    /// One of the command parameters was invalid or out of range.
    InvalidParameter,
    /// The requested address does not exist on the device.
    InvalidAddress,
    /// Attempt to write to a read-only register.
    ///
    /// A permanent condition: retrying cannot make it succeed.
    WriteProtect,
    /// The access was not aligned as the underlying technology requires.
    BadAlignment,
    /// Attempt to read a non-readable, or write a non-writable, register.
    ///
    /// Distinct from [`StatusCode::WriteProtect`]: the register accepts
    /// writes in principle but the device is refusing this one, typically
    /// because a GenApi lock is engaged or control privilege is not held.
    AccessDenied,
    /// The device is busy and the command may succeed if retried.
    ///
    /// The only status in this table worth a retry.
    Busy,
    /// The device reported a generic error with nothing more specific.
    GenericError,
    /// A status code not known to this implementation, or specific to one
    /// transport. Carries the raw value so a caller can still report it.
    Unknown(u16),
}

impl StatusCode {
    /// Convert from the raw status field in an acknowledgement header.
    pub fn from_raw(raw: u16) -> Self {
        match raw {
            0x0000 => StatusCode::Success,
            0x8001 => StatusCode::NotImplemented,
            0x8002 => StatusCode::InvalidParameter,
            0x8003 => StatusCode::InvalidAddress,
            0x8004 => StatusCode::WriteProtect,
            0x8005 => StatusCode::BadAlignment,
            0x8006 => StatusCode::AccessDenied,
            0x8007 => StatusCode::Busy,
            0x8FFF => StatusCode::GenericError,
            other => StatusCode::Unknown(other),
        }
    }

    /// Convert to the raw value stored in the packet header.
    pub const fn to_raw(self) -> u16 {
        match self {
            StatusCode::Success => 0x0000,
            StatusCode::NotImplemented => 0x8001,
            StatusCode::InvalidParameter => 0x8002,
            StatusCode::InvalidAddress => 0x8003,
            StatusCode::WriteProtect => 0x8004,
            StatusCode::BadAlignment => 0x8005,
            StatusCode::AccessDenied => 0x8006,
            StatusCode::Busy => 0x8007,
            StatusCode::GenericError => 0x8FFF,
            StatusCode::Unknown(code) => code,
        }
    }

    /// The specification's name for this code, for diagnostics.
    pub const fn name(self) -> &'static str {
        match self {
            StatusCode::Success => "SUCCESS",
            StatusCode::NotImplemented => "NOT_IMPLEMENTED",
            StatusCode::InvalidParameter => "INVALID_PARAMETER",
            StatusCode::InvalidAddress => "INVALID_ADDRESS",
            StatusCode::WriteProtect => "WRITE_PROTECT",
            StatusCode::BadAlignment => "BAD_ALIGNMENT",
            StatusCode::AccessDenied => "ACCESS_DENIED",
            StatusCode::Busy => "BUSY",
            StatusCode::GenericError => "ERROR",
            StatusCode::Unknown(_) => "unknown status",
        }
    }

    /// Whether retrying the command could plausibly succeed.
    ///
    /// True only for [`StatusCode::Busy`]. Notably *not* `WriteProtect` or
    /// `AccessDenied`, which are refusals rather than congestion.
    pub const fn is_retryable(self) -> bool {
        matches!(self, StatusCode::Busy)
    }
}

/// Prints the specification name **and** the raw hex value.
///
/// Both halves matter: the name is what makes an error actionable, and the
/// raw value is what lets a reporter match it against a capture. A bare
/// decimal — which is what `{:?}` on the old `Unknown(32774)` produced —
/// sent a user to the issue tracker in #45 unable to tell us anything.
impl std::fmt::Display for StatusCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (0x{:04X})", self.name(), self.to_raw())
    }
}

/// Errors that can occur when dealing with GenCP packets.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum GenCpError {
    #[error("invalid packet: {0}")]
    InvalidPacket(&'static str),
    #[error("unknown opcode: {0:#06x}")]
    UnknownOpcode(u16),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Command header for GenCP requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandHeader {
    /// Request flags (ack required, broadcast, …).
    pub flags: CommandFlags,
    /// Operation code for the request.
    pub opcode: OpCode,
    /// Length of the payload in bytes.
    pub length: u16,
    /// Request identifier chosen by the client.
    pub request_id: u16,
}

/// Header for GenCP acknowledgements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AckHeader {
    /// Status returned by the device.
    pub status: StatusCode,
    /// Operation code associated with the acknowledgement.
    pub opcode: OpCode,
    /// Length of the payload in bytes.
    pub length: u16,
    /// Request identifier that this acknowledgement answers.
    pub request_id: u16,
}

/// GenCP command packet.
#[derive(Debug, Clone)]
pub struct GenCpCmd {
    /// Packet header fields.
    pub header: CommandHeader,
    /// Command payload.
    pub payload: Bytes,
}

/// GenCP acknowledgement packet.
#[derive(Debug, Clone)]
pub struct GenCpAck {
    /// Header fields returned by the device.
    pub header: AckHeader,
    /// Payload data (command specific).
    pub payload: Bytes,
}

/// Encode a GenCP command into the on-the-wire representation.
///
/// The returned buffer is ready to be transmitted by the transport layer.
pub fn encode_cmd(cmd: &GenCpCmd) -> Bytes {
    debug_assert_eq!(cmd.header.length as usize, cmd.payload.len());
    let mut buffer = BytesMut::with_capacity(HEADER_SIZE + cmd.payload.len());
    buffer.put_u16(cmd.header.flags.bits());
    buffer.put_u16(cmd.header.opcode.command_code());
    buffer.put_u16(cmd.header.length);
    buffer.put_u16(cmd.header.request_id);
    buffer.extend_from_slice(&cmd.payload);
    buffer.freeze()
}

/// Decode a GenCP acknowledgement from raw bytes.
pub fn decode_ack(buf: &[u8]) -> Result<GenCpAck, GenCpError> {
    if buf.len() < HEADER_SIZE {
        return Err(GenCpError::InvalidPacket("too short"));
    }
    let mut cursor = buf;
    let status_raw = cursor.get_u16();
    let opcode_raw = cursor.get_u16();
    let length = cursor.get_u16();
    let request_id = cursor.get_u16();

    let expected = HEADER_SIZE + length as usize;
    if buf.len() != expected {
        return Err(GenCpError::InvalidPacket("length mismatch"));
    }

    let opcode = OpCode::from_ack(opcode_raw)?;
    let status = StatusCode::from_raw(status_raw);

    let payload = Bytes::copy_from_slice(&buf[HEADER_SIZE..]);
    Ok(GenCpAck {
        header: AckHeader {
            status,
            opcode,
            length,
            request_id,
        },
        payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shared status table, written as literal values rather than derived
    /// from `to_raw` — per ADR-0019, a fixture that reuses our own encoder
    /// cannot catch our own misreading. Cross-checked against Wireshark's
    /// `GEV_STATUS_*` defines and aravis's `ArvUvcpStatus`.
    const SPEC_STATUS_TABLE: &[(u16, StatusCode, &str)] = &[
        (0x0000, StatusCode::Success, "SUCCESS"),
        (0x8001, StatusCode::NotImplemented, "NOT_IMPLEMENTED"),
        (0x8002, StatusCode::InvalidParameter, "INVALID_PARAMETER"),
        (0x8003, StatusCode::InvalidAddress, "INVALID_ADDRESS"),
        (0x8004, StatusCode::WriteProtect, "WRITE_PROTECT"),
        (0x8005, StatusCode::BadAlignment, "BAD_ALIGNMENT"),
        (0x8006, StatusCode::AccessDenied, "ACCESS_DENIED"),
        (0x8007, StatusCode::Busy, "BUSY"),
        (0x8FFF, StatusCode::GenericError, "ERROR"),
    ];

    #[test]
    fn status_table_matches_the_specification() {
        for &(raw, expected, name) in SPEC_STATUS_TABLE {
            assert_eq!(
                StatusCode::from_raw(raw),
                expected,
                "decoding {raw:#06x} ({name})"
            );
            assert_eq!(expected.to_raw(), raw, "re-encoding {name}");
            assert_eq!(expected.name(), name);
        }
    }

    #[test]
    fn regression_the_three_codes_we_used_to_mislabel() {
        // 0x8004 was `DeviceBusy`, so a write to a read-only register reported
        // "device busy" and the GVCP retry loop kept retrying it.
        assert_eq!(StatusCode::from_raw(0x8004), StatusCode::WriteProtect);
        assert!(!StatusCode::from_raw(0x8004).is_retryable());

        // 0x8005 was the catch-all `Error`.
        assert_eq!(StatusCode::from_raw(0x8005), StatusCode::BadAlignment);

        // 0x8006 had no variant at all: #45's FLIR returned it on a locked
        // node and the user saw `Unknown(32774)` — 32774 being 0x8006.
        assert_eq!(StatusCode::from_raw(0x8006), StatusCode::AccessDenied);
        assert_eq!(StatusCode::from_raw(32774), StatusCode::AccessDenied);

        // 0x8007 is the one status a retry can help.
        assert!(StatusCode::from_raw(0x8007).is_retryable());
        assert!(!StatusCode::from_raw(0x0000).is_retryable());
    }

    #[test]
    fn display_carries_both_the_name_and_the_raw_value() {
        assert_eq!(
            StatusCode::AccessDenied.to_string(),
            "ACCESS_DENIED (0x8006)"
        );
        // An unrecognised code still prints as hex, never bare decimal.
        assert_eq!(
            StatusCode::from_raw(0x800C).to_string(),
            "unknown status (0x800C)"
        );
    }

    #[test]
    fn transport_specific_codes_stay_unknown_here() {
        // 0x800B is the reason this enum holds only the shared core: GVCP
        // calls it NO_MSG (deprecated), GenCP calls it MSG_TIMEOUT. Decoding
        // it here would have to pick one and be wrong for the other
        // transport, so it is deliberately left to the transport (ADR-0020).
        assert_eq!(StatusCode::from_raw(0x800B), StatusCode::Unknown(0x800B));
        // Likewise the GVCP packet-resend family and the GenCP 0xA0xx range.
        assert_eq!(StatusCode::from_raw(0x800C), StatusCode::Unknown(0x800C));
        assert_eq!(StatusCode::from_raw(0xA001), StatusCode::Unknown(0xA001));
        // Round-tripping an unknown code must not lose it.
        assert_eq!(StatusCode::from_raw(0x800B).to_raw(), 0x800B);
    }

    #[test]
    fn pending_ack_is_a_command_id_not_a_status() {
        // The bug this constant replaces: 0x8006 was treated as "pending".
        assert_eq!(PENDING_ACK_COMMAND, 0x0805);
        assert_ne!(PENDING_ACK_COMMAND, StatusCode::AccessDenied.to_raw());
        // It sits in the command-id table beside the ones we already model.
        assert_eq!(OpCode::ReadMem.command_code(), 0x0084);
        assert_eq!(OpCode::WriteMem.command_code(), 0x0086);
    }

    #[test]
    fn encode_read_register_roundtrip() {
        let payload = {
            let mut p = BytesMut::with_capacity(4);
            p.put_u32(0x0000_0a00);
            p.freeze()
        };
        let cmd = GenCpCmd {
            header: CommandHeader {
                flags: CommandFlags::ACK_REQUIRED,
                opcode: OpCode::ReadRegister,
                length: payload.len() as u16,
                request_id: 0x41,
            },
            payload,
        };

        let encoded = encode_cmd(&cmd);
        assert_eq!(
            &encoded[..2],
            &CommandFlags::ACK_REQUIRED.bits().to_be_bytes()
        );
        assert_eq!(&encoded[2..4], &0x0080u16.to_be_bytes());
        assert_eq!(&encoded[4..6], &(cmd.payload.len() as u16).to_be_bytes());
        assert_eq!(&encoded[6..8], &0x0041u16.to_be_bytes());
        assert_eq!(&encoded[8..], &cmd.payload[..]);
    }

    #[test]
    fn encode_write_register_roundtrip() {
        let payload = {
            let mut p = BytesMut::with_capacity(8);
            p.put_u32(0x0000_0a00);
            p.put_u32(0x0000_0002);
            p.freeze()
        };
        let cmd = GenCpCmd {
            header: CommandHeader {
                flags: CommandFlags::ACK_REQUIRED,
                opcode: OpCode::WriteRegister,
                length: payload.len() as u16,
                request_id: 0x43,
            },
            payload,
        };

        let encoded = encode_cmd(&cmd);
        assert_eq!(
            &encoded[..2],
            &CommandFlags::ACK_REQUIRED.bits().to_be_bytes()
        );
        assert_eq!(&encoded[2..4], &0x0082u16.to_be_bytes());
        assert_eq!(&encoded[4..6], &(cmd.payload.len() as u16).to_be_bytes());
        assert_eq!(&encoded[6..8], &0x0043u16.to_be_bytes());
        assert_eq!(&encoded[8..], &cmd.payload[..]);
    }

    #[test]
    fn decode_read_register_ack() {
        let value = 0x0000_0002u32;
        let mut buf = BytesMut::with_capacity(HEADER_SIZE + 4);
        buf.put_u16(0x0000);
        buf.put_u16(0x0081);
        buf.put_u16(4);
        buf.put_u16(0x4141);
        buf.put_u32(value);

        let ack = decode_ack(&buf).expect("decode");
        assert_eq!(ack.header.status, StatusCode::Success);
        assert_eq!(ack.header.opcode, OpCode::ReadRegister);
        assert_eq!(ack.header.length, 4);
        assert_eq!(ack.header.request_id, 0x4141);
        assert_eq!(&ack.payload[..], &value.to_be_bytes());
    }

    #[test]
    fn decode_write_register_ack() {
        let index = 1u32;
        let mut buf = BytesMut::with_capacity(HEADER_SIZE + 4);
        buf.put_u16(0x0000);
        buf.put_u16(0x0083);
        buf.put_u16(4);
        buf.put_u16(0x4343);
        buf.put_u32(index);

        let ack = decode_ack(&buf).expect("decode");
        assert_eq!(ack.header.status, StatusCode::Success);
        assert_eq!(ack.header.opcode, OpCode::WriteRegister);
        assert_eq!(ack.header.length, 4);
        assert_eq!(ack.header.request_id, 0x4343);
        assert_eq!(&ack.payload[..], &index.to_be_bytes());
    }

    #[test]
    fn encode_read_mem_roundtrip() {
        let payload = {
            let mut p = BytesMut::with_capacity(12);
            p.put_u64(0x0010_0200);
            p.put_u32(64);
            p.freeze()
        };
        let cmd = GenCpCmd {
            header: CommandHeader {
                flags: CommandFlags::ACK_REQUIRED,
                opcode: OpCode::ReadMem,
                length: payload.len() as u16,
                request_id: 0x42,
            },
            payload,
        };

        let encoded = encode_cmd(&cmd);
        assert_eq!(
            &encoded[..2],
            &CommandFlags::ACK_REQUIRED.bits().to_be_bytes()
        );
        assert_eq!(&encoded[2..4], &0x0084u16.to_be_bytes());
        assert_eq!(&encoded[4..6], &(cmd.payload.len() as u16).to_be_bytes());
        assert_eq!(&encoded[6..8], &0x0042u16.to_be_bytes());
        assert_eq!(&encoded[8..], &cmd.payload[..]);
    }

    #[test]
    fn decode_read_mem_ack() {
        let payload = vec![0xAA; 4];
        let mut buf = BytesMut::with_capacity(HEADER_SIZE + payload.len());
        buf.put_u16(0x0000);
        buf.put_u16(0x0085);
        buf.put_u16(payload.len() as u16);
        buf.put_u16(0x4242);
        buf.extend_from_slice(&payload);

        let ack = decode_ack(&buf).expect("decode");
        assert_eq!(ack.header.status, StatusCode::Success);
        assert_eq!(ack.header.opcode, OpCode::ReadMem);
        assert_eq!(ack.header.length as usize, payload.len());
        assert_eq!(ack.header.request_id, 0x4242);
        assert_eq!(&ack.payload[..], &payload[..]);
    }

    #[test]
    fn decode_write_mem_ack() {
        let payload: Vec<u8> = Vec::new();
        let mut buf = BytesMut::with_capacity(HEADER_SIZE + payload.len());
        buf.put_u16(0x0000);
        buf.put_u16(0x0087);
        buf.put_u16(0);
        buf.put_u16(0x1001);
        let ack = decode_ack(&buf).expect("decode");
        assert_eq!(ack.header.opcode, OpCode::WriteMem);
        assert_eq!(ack.header.status, StatusCode::Success);
        assert_eq!(ack.payload.len(), 0);
    }

    // ── Spec-derived acknowledgement header (backlog TC-04) ────────────────
    //
    // The tests above build their input with the same `put_u16` calls the
    // encoder uses, in the order the decoder reads them. That proves the two
    // agree; it cannot show either matches the standard. These assert the
    // header as a literal byte array written from the specification's field
    // table, and index it by offset.
    //
    // | Offset | Size | Field                    |
    // |--------|------|--------------------------|
    // |      0 |    2 | Status                   |
    // |      2 |    2 | Acknowledge command id   |
    // |      4 |    2 | Length of the payload    |
    // |      6 |    2 | Request id (echoed)      |
    // |      8 |    n | Payload                  |
    //
    // All fields are big-endian.

    /// A `READREG_ACK` returning `0x0000_3EF2`, byte for byte.
    const GOLDEN_READ_REGISTER_ACK: [u8; 12] = [
        0x00, 0x00, // status: SUCCESS
        0x00, 0x81, // acknowledge command id: READREG_ACK
        0x00, 0x04, // length: 4 — the payload alone
        0x12, 0x34, // request id
        0x00, 0x00, 0x3E, 0xF2, // payload: 16114
    ];

    #[test]
    fn ack_header_fields_sit_at_the_specified_offsets() {
        let b = &GOLDEN_READ_REGISTER_ACK;
        assert_eq!(u16::from_be_bytes([b[0], b[1]]), 0x0000, "status at 0");
        assert_eq!(u16::from_be_bytes([b[2], b[3]]), 0x0081, "ack id at 2");
        assert_eq!(u16::from_be_bytes([b[4], b[5]]), 4, "length at 4");
        assert_eq!(u16::from_be_bytes([b[6], b[7]]), 0x1234, "request id at 6");
        assert_eq!(HEADER_SIZE, 8, "the payload begins at offset 8");

        let ack = decode_ack(b).expect("decode the golden ack");
        assert_eq!(ack.header.status, StatusCode::Success);
        assert_eq!(ack.header.opcode, OpCode::ReadRegister);
        assert_eq!(ack.header.length, 4);
        assert_eq!(ack.header.request_id, 0x1234);
        assert_eq!(&ack.payload[..], &[0x00, 0x00, 0x3E, 0xF2]);
    }

    /// `length` counts the payload, not the whole datagram.
    ///
    /// Off by exactly `HEADER_SIZE`, a fake and a client still round-trip
    /// perfectly with each other while every real device disagrees — the
    /// ADR-0019 shape. Pin the interpretation rather than the arithmetic.
    #[test]
    fn ack_length_counts_the_payload_only() {
        let mut counts_the_header = GOLDEN_READ_REGISTER_ACK;
        counts_the_header[4..6].copy_from_slice(&12u16.to_be_bytes());
        assert!(
            decode_ack(&counts_the_header).is_err(),
            "a length of 12 describes a 20-byte datagram, not this one"
        );

        // And the truncation case: the field promises more than arrived.
        let mut over_promises = GOLDEN_READ_REGISTER_ACK;
        over_promises[4..6].copy_from_slice(&8u16.to_be_bytes());
        assert!(decode_ack(&over_promises).is_err());
    }

    /// The acknowledge command id is the command id plus one, and the decoder
    /// must reject a *command* id arriving where an acknowledgement belongs.
    #[test]
    fn ack_ids_are_command_ids_plus_one() {
        for (cmd, ack) in [
            (0x0080u16, 0x0081u16),
            (0x0082, 0x0083),
            (0x0084, 0x0085),
            (0x0086, 0x0087),
        ] {
            assert_eq!(
                OpCode::from_command(cmd).expect("command id").ack_code(),
                ack
            );
            assert!(
                OpCode::from_ack(cmd).is_err(),
                "{cmd:#06x} is a command id, not an acknowledgement"
            );
        }
    }

    /// A pending-acknowledge is a distinct command id carrying a *success*
    /// status, so nothing about the status word distinguishes it from the real
    /// answer (backlog TC-16). It must not decode as one.
    #[test]
    fn pending_ack_is_not_a_normal_acknowledgement() {
        let mut pending = GOLDEN_READ_REGISTER_ACK;
        pending[2..4].copy_from_slice(&PENDING_ACK_COMMAND.to_be_bytes());
        assert_eq!(
            u16::from_be_bytes([pending[0], pending[1]]),
            0x0000,
            "a pending-ack reports SUCCESS — the status cannot be the signal"
        );
        assert!(
            decode_ack(&pending).is_err(),
            "0x0805 is not an acknowledgement command id"
        );
    }

    /// The command header uses the same four fields in the same order, so the
    /// encoder is pinned by offset too.
    #[test]
    fn command_header_fields_sit_at_the_specified_offsets() {
        let cmd = GenCpCmd {
            header: CommandHeader {
                flags: CommandFlags::ACK_REQUIRED,
                opcode: OpCode::ReadMem,
                length: 8,
                request_id: 0x00AB,
            },
            payload: Bytes::from_static(&[0, 0, 0x0D, 0x04, 0, 0, 0, 4]),
        };
        let b = encode_cmd(&cmd);

        assert_eq!(u16::from_be_bytes([b[0], b[1]]), 0x0001, "flags at 0");
        assert_eq!(u16::from_be_bytes([b[2], b[3]]), 0x0084, "READMEM_CMD at 2");
        assert_eq!(u16::from_be_bytes([b[4], b[5]]), 8, "payload length at 4");
        assert_eq!(u16::from_be_bytes([b[6], b[7]]), 0x00AB, "request id at 6");
        assert_eq!(b.len(), HEADER_SIZE + 8);
    }
}
