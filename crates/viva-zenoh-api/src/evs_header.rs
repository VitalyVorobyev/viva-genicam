//! Binary header prepended to every event-vision block on the Zenoh `evs` key.
//!
//! An event-vision camera — a LUCID Triton2 EVS, for one
//! ([#138](https://github.com/VitalyVorobyev/viva-genicam/issues/138)) —
//! streams encoded events, not pixels. Its blocks are published on
//! [`keys::evs`](crate::keys::evs) rather than [`keys::image`](crate::keys::image),
//! framed the way images are: a fixed little-endian header, then the payload
//! exactly as the device sent it.
//!
//! ## Layout (24 bytes, all fields little-endian)
//!
//! | Offset | Size | Field       | Value / Notes                                   |
//! |--------|------|-------------|-------------------------------------------------|
//! | 0      | 2    | magic       | `0x5645` LE (`[0x45, 0x56]`, `"EV"`)            |
//! | 2      | 1    | version     | `1`                                             |
//! | 3      | 1    | format      | event encoding discriminant (see [`EvsFormat`]) |
//! | 4      | 4    | seq         | per-acquisition block counter, u32              |
//! | 8      | 8    | timestamp   | device timestamp; meaningful only if flagged    |
//! | 16     | 4    | payload_len | length of the event bytes that follow, u32      |
//! | 20     | 1    | flags       | bit 0: `timestamp` is present                   |
//! | 21     | 3    | reserved    | zero                                            |

use thiserror::Error;

/// Magic bytes identifying an event-vision block header.
/// Little-endian encoding of `0x5645` -> bytes `[0x45, 0x56]` (`"EV"`).
pub const EVS_MAGIC: u16 = 0x5645;

/// Fixed size of the binary event-vision block header in bytes.
pub const EVS_HEADER_SIZE: usize = 24;

/// The only supported header version. Bumped when the layout changes.
pub const EVS_SUPPORTED_VERSION: u8 = 1;

const FLAG_TIMESTAMP: u8 = 0x01;

/// Encoding of the event bytes that follow an [`EvsHeader`].
///
/// Codes are only appended, never reordered; a code this build does not know
/// decodes to [`EvsFormat::Unknown`] rather than failing, so a newer service
/// does not break an older client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EvsFormat {
    /// An encoding this build does not know. Wire code `0`.
    Unknown,
    /// Prophesee EVT 3.0: 16-bit words. Wire code `1`.
    Evt30,
    /// Prophesee EVT 2.1: 64-bit events. Wire code `2`.
    Evt21,
}

impl EvsFormat {
    /// The stable one-byte wire code.
    pub fn code(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::Evt30 => 1,
            Self::Evt21 => 2,
        }
    }

    /// The format for a wire code; [`EvsFormat::Unknown`] for any code this
    /// build does not know.
    pub fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Evt30,
            2 => Self::Evt21,
            _ => Self::Unknown,
        }
    }

    /// A short human-readable name: `"EVT3.0"`, `"EVT2.1"` or `"Unknown"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::Evt30 => "EVT3.0",
            Self::Evt21 => "EVT2.1",
        }
    }
}

/// Errors returned by [`EvsHeader::decode`].
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum EvsHeaderError {
    /// Buffer is shorter than [`EVS_HEADER_SIZE`] bytes.
    #[error("buffer too short: need {EVS_HEADER_SIZE} bytes, got {0}")]
    TooShort(usize),

    /// First two bytes do not match [`EVS_MAGIC`].
    #[error("bad magic: expected 0x{:04X}, got 0x{got:04X}", EVS_MAGIC)]
    BadMagic {
        /// The two bytes found, read little-endian.
        got: u16,
    },

    /// Version byte is not [`EVS_SUPPORTED_VERSION`].
    #[error("unsupported version {0}; only version {EVS_SUPPORTED_VERSION} is supported")]
    UnsupportedVersion(u8),

    /// The header's `payload_len` disagrees with the bytes that follow it.
    #[error("header declares {declared} payload bytes, {actual} follow")]
    LengthMismatch {
        /// `payload_len` from the header.
        declared: u32,
        /// Bytes actually following the header.
        actual: usize,
    },
}

/// 24-byte header prepended to every event-vision block.
///
/// See the module-level documentation for the byte layout.
///
/// `#[non_exhaustive]`: build one with [`EvsHeader::new`], so that adding the
/// next field is not a breaking change.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EvsHeader {
    /// Encoding of the event bytes.
    pub format: EvsFormat,
    /// Block counter, starting at zero for each acquisition (wraps at
    /// `u32::MAX`). Counts event blocks only; images have their own `seq`.
    pub seq: u32,
    /// Device timestamp from the GVSP leader, when the stream carried one.
    pub timestamp: Option<u64>,
    /// Length in bytes of the event data that follows the header.
    pub payload_len: u32,
}

impl EvsHeader {
    /// A header for `payload_len` bytes of `format` events.
    pub fn new(format: EvsFormat, seq: u32, timestamp: Option<u64>, payload_len: u32) -> Self {
        Self {
            format,
            seq,
            timestamp,
            payload_len,
        }
    }

    /// Encode this header into a 24-byte `Vec<u8>`.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(EVS_HEADER_SIZE);
        buf.extend_from_slice(&EVS_MAGIC.to_le_bytes());
        buf.push(EVS_SUPPORTED_VERSION);
        buf.push(self.format.code());
        buf.extend_from_slice(&self.seq.to_le_bytes());
        buf.extend_from_slice(&self.timestamp.unwrap_or(0).to_le_bytes());
        buf.extend_from_slice(&self.payload_len.to_le_bytes());
        buf.push(if self.timestamp.is_some() {
            FLAG_TIMESTAMP
        } else {
            0
        });
        buf.extend_from_slice(&[0; 3]);
        buf
    }

    /// Decode a header from the front of `buf`.
    ///
    /// On success returns `(header, event_bytes)`, where `event_bytes` is the
    /// remainder after the header and is exactly `payload_len` long.
    pub fn decode(buf: &[u8]) -> Result<(EvsHeader, &[u8]), EvsHeaderError> {
        if buf.len() < EVS_HEADER_SIZE {
            return Err(EvsHeaderError::TooShort(buf.len()));
        }

        let magic = u16::from_le_bytes([buf[0], buf[1]]);
        if magic != EVS_MAGIC {
            return Err(EvsHeaderError::BadMagic { got: magic });
        }

        let version = buf[2];
        if version != EVS_SUPPORTED_VERSION {
            return Err(EvsHeaderError::UnsupportedVersion(version));
        }

        let format = EvsFormat::from_code(buf[3]);
        let seq = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let mut ts = [0u8; 8];
        ts.copy_from_slice(&buf[8..16]);
        let payload_len = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]);
        let timestamp = (buf[20] & FLAG_TIMESTAMP != 0).then(|| u64::from_le_bytes(ts));

        let events = &buf[EVS_HEADER_SIZE..];
        if events.len() != payload_len as usize {
            return Err(EvsHeaderError::LengthMismatch {
                declared: payload_len,
                actual: events.len(),
            });
        }

        Ok((EvsHeader::new(format, seq, timestamp, payload_len), events))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed(header: &EvsHeader, events: &[u8]) -> Vec<u8> {
        let mut buf = header.encode();
        buf.extend_from_slice(events);
        buf
    }

    #[test]
    fn roundtrip_with_and_without_timestamp() {
        let events = [1u8, 2, 3, 4, 5, 6];
        for timestamp in [Some(0), Some(0x0123_4567_89AB_CDEF), None] {
            let header = EvsHeader::new(EvsFormat::Evt30, 9, timestamp, events.len() as u32);
            let buf = framed(&header, &events);
            assert_eq!(buf.len(), EVS_HEADER_SIZE + events.len());
            let (decoded, rest) = EvsHeader::decode(&buf).expect("decode");
            assert_eq!(decoded, header);
            assert_eq!(rest, events);
        }
    }

    #[test]
    fn magic_is_distinct_from_the_image_header() {
        assert_ne!(EVS_MAGIC, crate::FRAME_MAGIC);
        let buf = EvsHeader::new(EvsFormat::Evt21, 0, None, 0).encode();
        assert_eq!(&buf[..2], b"EV");
        assert!(crate::FrameHeader::decode(&buf).is_err());
    }

    #[test]
    fn format_codes_are_stable_and_unknown_codes_degrade() {
        for format in [EvsFormat::Unknown, EvsFormat::Evt30, EvsFormat::Evt21] {
            assert_eq!(EvsFormat::from_code(format.code()), format);
        }
        assert_eq!(EvsFormat::Evt30.code(), 1);
        assert_eq!(EvsFormat::Evt21.code(), 2);
        assert_eq!(EvsFormat::from_code(0xFF), EvsFormat::Unknown);
    }

    #[test]
    fn decode_rejects_malformed_buffers() {
        assert_eq!(
            EvsHeader::decode(&[0; 23]).unwrap_err(),
            EvsHeaderError::TooShort(23)
        );

        let good = framed(&EvsHeader::new(EvsFormat::Evt30, 0, None, 2), &[7, 7]);

        let mut bad_magic = good.clone();
        bad_magic[0] = 0;
        assert!(matches!(
            EvsHeader::decode(&bad_magic).unwrap_err(),
            EvsHeaderError::BadMagic { .. }
        ));

        let mut bad_version = good.clone();
        bad_version[2] = 2;
        assert_eq!(
            EvsHeader::decode(&bad_version).unwrap_err(),
            EvsHeaderError::UnsupportedVersion(2)
        );

        assert_eq!(
            EvsHeader::decode(&good[..good.len() - 1]).unwrap_err(),
            EvsHeaderError::LengthMismatch {
                declared: 2,
                actual: 1
            }
        );
    }
}
