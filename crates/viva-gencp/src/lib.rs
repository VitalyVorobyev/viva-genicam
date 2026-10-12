#![cfg_attr(docsrs, feature(doc_cfg))]
//! GenCP: generic control protocol encode/decode (transport-agnostic).

use bitflags::bitflags;
use bytes::{Buf, Bytes};
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
/// (`viva_gige::consts::PENDING_ACK`).
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
/// Only the codes **both** protocols define, under the same name and meaning,
/// live here: `0x0000`, `0x8001`–`0x8007`, `0x800E`, `0x800F` and `0x8FFF`.
/// Elsewhere the tables diverge, and at `0x800B` they actively disagree —
/// GVCP calls it `NO_MSG` (deprecated), GenCP calls it `MSG_TIMEOUT` — so
/// each transport decodes its acknowledgements with a status type of its own
/// that wraps this one: `viva_gige::GigeStatus` and `viva_u3v::U3vStatus`
/// (ADR-0020).
///
/// There is no `Unknown` here: [`StatusCode::from_raw`] returns `None` for a
/// code outside the shared core, and the transport's own type names it or
/// keeps it as unknown.
///
/// Values corroborated by Wireshark's GVCP dissector
/// (`epan/dissectors/packet-gvcp.c`, `GEV_STATUS_*`) and aravis
/// (`arvgvcpprivate.h`, `arvgvspprivate.h`, `arvuvcpprivate.h`), which agree
/// with each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
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
    /// The command's header is invalid: its fields, or its length against
    /// the bytes that arrived.
    InvalidHeader,
    /// The device's current configuration does not allow the command.
    /// Deprecated in GigE Vision.
    WrongConfig,
    /// The device reported a generic error with nothing more specific.
    GenericError,
}

impl StatusCode {
    /// The shared meaning of a raw status, or `None` for a code outside the
    /// shared core — one a transport defines, or one nobody does.
    pub const fn from_raw(raw: u16) -> Option<Self> {
        Some(match raw {
            0x0000 => StatusCode::Success,
            0x8001 => StatusCode::NotImplemented,
            0x8002 => StatusCode::InvalidParameter,
            0x8003 => StatusCode::InvalidAddress,
            0x8004 => StatusCode::WriteProtect,
            0x8005 => StatusCode::BadAlignment,
            0x8006 => StatusCode::AccessDenied,
            0x8007 => StatusCode::Busy,
            0x800E => StatusCode::InvalidHeader,
            0x800F => StatusCode::WrongConfig,
            0x8FFF => StatusCode::GenericError,
            _ => return None,
        })
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
            StatusCode::InvalidHeader => 0x800E,
            StatusCode::WrongConfig => 0x800F,
            StatusCode::GenericError => 0x8FFF,
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
            StatusCode::InvalidHeader => "INVALID_HEADER",
            StatusCode::WrongConfig => "WRONG_CONFIG",
            StatusCode::GenericError => "ERROR",
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

/// Defines a transport's status type: the shared [`StatusCode`] core, the
/// transport's own codes, and `Unknown` for the rest (ADR-0020).
///
/// Used by `viva-gige` and `viva-u3v`, so the two types are built the same
/// way and print the same way: the name and the raw hex, never bare decimal.
#[macro_export]
#[doc(hidden)]
macro_rules! transport_status {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $(
                $(#[$variant_meta:meta])*
                $variant:ident = $raw:literal, $spec:literal;
            )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        $vis enum $name {
            /// A code GenCP and GigE Vision share: see
            /// `viva_gencp::StatusCode`.
            Common($crate::StatusCode),
            $(
                $(#[$variant_meta])*
                $variant,
            )*
            /// A code neither this transport's table nor the shared core
            /// names. Carries the raw value so it can still be reported.
            Unknown(u16),
        }

        impl $name {
            /// Decode the raw status field of an acknowledgement.
            pub const fn from_raw(raw: u16) -> Self {
                match raw {
                    $($raw => $name::$variant,)*
                    _ => match $crate::StatusCode::from_raw(raw) {
                        Some(common) => $name::Common(common),
                        None => $name::Unknown(raw),
                    },
                }
            }

            /// The raw value, as the acknowledgement carries it.
            pub const fn to_raw(self) -> u16 {
                match self {
                    $name::Common(common) => common.to_raw(),
                    $($name::$variant => $raw,)*
                    $name::Unknown(raw) => raw,
                }
            }

            /// The specification's name for this code, for diagnostics.
            pub const fn name(self) -> &'static str {
                match self {
                    $name::Common(common) => common.name(),
                    $($name::$variant => $spec,)*
                    $name::Unknown(_) => "unknown status",
                }
            }

            /// The shared meaning, for a code in the core both transports
            /// define.
            pub const fn common(self) -> Option<$crate::StatusCode> {
                match self {
                    $name::Common(common) => Some(common),
                    _ => None,
                }
            }

            /// Whether this is `SUCCESS`.
            pub const fn is_success(self) -> bool {
                matches!(self, $name::Common($crate::StatusCode::Success))
            }

            /// Whether retrying the command could plausibly succeed: only
            /// `BUSY`.
            pub const fn is_retryable(self) -> bool {
                matches!(self, $name::Common($crate::StatusCode::Busy))
            }
        }

        impl From<$crate::StatusCode> for $name {
            fn from(common: $crate::StatusCode) -> Self {
                $name::Common(common)
            }
        }

        /// Prints the specification name and the raw hex value.
        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                write!(f, "{} (0x{:04X})", self.name(), self.to_raw())
            }
        }
    };
}

/// Errors that can occur when dealing with GenCP packets.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum GenCpError {
    #[error("invalid packet: {0}")]
    /// The packet is malformed: too short, or its length field disagrees.
    InvalidPacket(&'static str),
    #[error("unknown opcode: {0:#06x}")]
    /// An acknowledgement carries a command id this crate does not know.
    UnknownOpcode(u16),
    #[error("io: {0}")]
    /// A socket operation failed.
    Io(#[from] std::io::Error),
}

/// Header of a GVCP acknowledgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AckHeader {
    /// Status returned by the device, raw: GigE Vision decodes it with its own
    /// table (`viva_gige::GigeStatus`), which [`StatusCode`] is the shared
    /// core of.
    pub status: u16,
    /// Operation code associated with the acknowledgement.
    pub opcode: OpCode,
    /// Length of the payload in bytes.
    pub length: u16,
    /// Request identifier that this acknowledgement answers.
    pub request_id: u16,
}

/// GenCP acknowledgement packet.
#[derive(Debug, Clone)]
pub struct GenCpAck {
    /// Header fields returned by the device.
    pub header: AckHeader,
    /// Payload data (command specific).
    pub payload: Bytes,
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

    let payload = Bytes::copy_from_slice(&buf[HEADER_SIZE..]);
    Ok(GenCpAck {
        header: AckHeader {
            status: status_raw,
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
    use bytes::{BufMut, BytesMut};

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
        (0x800E, StatusCode::InvalidHeader, "INVALID_HEADER"),
        (0x800F, StatusCode::WrongConfig, "WRONG_CONFIG"),
        (0x8FFF, StatusCode::GenericError, "ERROR"),
    ];

    #[test]
    fn status_table_matches_the_specification() {
        for &(raw, expected, name) in SPEC_STATUS_TABLE {
            assert_eq!(
                StatusCode::from_raw(raw),
                Some(expected),
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
        assert_eq!(StatusCode::from_raw(0x8004), Some(StatusCode::WriteProtect));
        assert!(!StatusCode::WriteProtect.is_retryable());

        // 0x8005 was the catch-all `Error`.
        assert_eq!(StatusCode::from_raw(0x8005), Some(StatusCode::BadAlignment));

        // 0x8006 had no variant at all: #45's FLIR returned it on a locked
        // node and the user saw `Unknown(32774)` — 32774 being 0x8006.
        assert_eq!(StatusCode::from_raw(0x8006), Some(StatusCode::AccessDenied));
        assert_eq!(StatusCode::from_raw(32774), Some(StatusCode::AccessDenied));

        // 0x8007 is the one status a retry can help.
        assert!(StatusCode::Busy.is_retryable());
        assert!(!StatusCode::Success.is_retryable());
    }

    #[test]
    fn display_carries_both_the_name_and_the_raw_value() {
        assert_eq!(
            StatusCode::AccessDenied.to_string(),
            "ACCESS_DENIED (0x8006)"
        );
    }

    #[test]
    fn transport_specific_codes_are_not_in_the_shared_core() {
        // 0x800B is the reason this enum holds only the shared core: GVCP
        // calls it NO_MSG (deprecated), GenCP calls it MSG_TIMEOUT. Decoding
        // it here would have to pick one and be wrong for the other
        // transport, so it is left to the transport's own type (ADR-0020).
        assert_eq!(StatusCode::from_raw(0x800B), None);
        // Likewise the GVCP packet-resend family and the USB3 Vision 0xA0xx
        // range.
        assert_eq!(StatusCode::from_raw(0x800C), None);
        assert_eq!(StatusCode::from_raw(0xA001), None);
    }

    // The macro's methods are `pub` for the transports' public types; on a
    // test's private one they are unreachable, which is the point.
    #[allow(unreachable_pub)]
    mod example {
        transport_status! {
            /// A transport with one code of its own, to check the macro.
            pub(crate) enum Example {
                /// `0x800B`, as the example transport names it.
                Mine = 0x800B, "MINE";
            }
        }
    }
    use example::Example;

    #[test]
    fn a_transport_status_wraps_the_core_and_keeps_the_rest() {
        assert_eq!(Example::from_raw(0x800B), Example::Mine);
        assert_eq!(
            Example::from_raw(0x8006),
            Example::Common(StatusCode::AccessDenied)
        );
        assert_eq!(Example::from_raw(0x8123), Example::Unknown(0x8123));
        for raw in [0x0000, 0x800B, 0x8006, 0x8123] {
            assert_eq!(Example::from_raw(raw).to_raw(), raw, "{raw:#06x}");
        }
        assert_eq!(Example::Mine.to_string(), "MINE (0x800B)");
        assert_eq!(
            Example::Unknown(0x8123).to_string(),
            "unknown status (0x8123)"
        );
        assert!(Example::from_raw(0x8007).is_retryable());
        assert!(Example::from_raw(0).is_success());
        assert_eq!(Example::Mine.common(), None);
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
    fn decode_read_register_ack() {
        let value = 0x0000_0002u32;
        let mut buf = BytesMut::with_capacity(HEADER_SIZE + 4);
        buf.put_u16(0x0000);
        buf.put_u16(0x0081);
        buf.put_u16(4);
        buf.put_u16(0x4141);
        buf.put_u32(value);

        let ack = decode_ack(&buf).expect("decode");
        assert_eq!(ack.header.status, StatusCode::Success.to_raw());
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
        assert_eq!(ack.header.status, StatusCode::Success.to_raw());
        assert_eq!(ack.header.opcode, OpCode::WriteRegister);
        assert_eq!(ack.header.length, 4);
        assert_eq!(ack.header.request_id, 0x4343);
        assert_eq!(&ack.payload[..], &index.to_be_bytes());
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
        assert_eq!(ack.header.status, StatusCode::Success.to_raw());
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
        assert_eq!(ack.header.status, StatusCode::Success.to_raw());
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
        assert_eq!(ack.header.status, StatusCode::Success.to_raw());
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
}
