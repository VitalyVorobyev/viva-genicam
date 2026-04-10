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
    }
}

/// GenCP operation codes supported by this crate.
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
    /// Raw command value as defined by the GenCP/GVCP specification.
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

/// Status codes returned by GenCP acknowledgements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusCode {
    /// Command completed successfully.
    Success,
    /// The requested command is not implemented by the device.
    NotImplemented,
    /// One of the command parameters was invalid.
    InvalidParameter,
    /// The requested address range cannot be accessed.
    InvalidAddress,
    /// The device was busy processing a previous command.
    DeviceBusy,
    /// The device reported a generic or transport specific error.
    Error,
    /// A status code not known to this implementation.
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
            0x8004 => StatusCode::DeviceBusy,
            0x8005 => StatusCode::Error,
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
            StatusCode::DeviceBusy => 0x8004,
            StatusCode::Error => 0x8005,
            StatusCode::Unknown(code) => code,
        }
    }
}

/// Errors that can occur when dealing with GenCP packets.
#[derive(Debug, Error)]
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
}
