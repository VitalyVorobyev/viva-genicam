//! Binary frame header prepended to every raw pixel buffer on the Zenoh image key.
//!
//! ## Layout (16 bytes, all fields little-endian)
//!
//! | Offset | Size | Field   | Value / Notes                          |
//! |--------|------|---------|----------------------------------------|
//! | 0      | 2    | magic   | `0x4746` LE (`[0x46, 0x47]`)           |
//! | 2      | 1    | version | `1`                                    |
//! | 3      | 1    | format  | pixel format discriminant (see table)  |
//! | 4      | 4    | width   | image width in pixels, u32 LE          |
//! | 8      | 4    | height  | image height in pixels, u32 LE         |
//! | 12     | 4    | seq     | monotonically increasing frame counter |

use thiserror::Error;

use crate::PixelFormat;

/// Magic bytes identifying a GenICam frame header.
/// Little-endian encoding of `0x4746` -> bytes `[0x46, 0x47]`.
pub const FRAME_MAGIC: u16 = 0x4746;

/// Fixed size of the binary frame header in bytes.
pub const HEADER_SIZE: usize = 16;

/// The only supported header version. Bumped when the layout changes.
pub const SUPPORTED_VERSION: u8 = 1;

/// Errors returned by [`FrameHeader::decode`].
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum FrameHeaderError {
    /// Buffer is shorter than [`HEADER_SIZE`] bytes.
    #[error("buffer too short: need {HEADER_SIZE} bytes, got {0}")]
    TooShort(usize),

    /// First two bytes do not match [`FRAME_MAGIC`].
    #[error("bad magic: expected 0x{:04X}, got 0x{got:04X}", FRAME_MAGIC)]
    BadMagic { got: u16 },

    /// Version byte is not [`SUPPORTED_VERSION`].
    #[error("unsupported version {0}; only version {SUPPORTED_VERSION} is supported")]
    UnsupportedVersion(u8),
}

/// 16-byte binary frame header prepended to every image payload.
///
/// See module-level documentation for the byte layout and format discriminant table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameHeader {
    /// Pixel format of the following pixel data.
    pub pixel_format: PixelFormat,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Monotonically increasing frame sequence number (wraps at u32::MAX).
    pub seq: u32,
}

impl FrameHeader {
    /// Encode this header into a 16-byte `Vec<u8>`.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(HEADER_SIZE);
        buf.extend_from_slice(&FRAME_MAGIC.to_le_bytes());
        buf.push(SUPPORTED_VERSION);
        buf.push(pixel_format_to_u8(&self.pixel_format));
        buf.extend_from_slice(&self.width.to_le_bytes());
        buf.extend_from_slice(&self.height.to_le_bytes());
        buf.extend_from_slice(&self.seq.to_le_bytes());
        buf
    }

    /// Decode a header from the front of `buf`.
    ///
    /// On success returns `(header, pixel_data_slice)` where `pixel_data_slice`
    /// is the remaining bytes after the 16-byte header.
    pub fn decode(buf: &[u8]) -> Result<(FrameHeader, &[u8]), FrameHeaderError> {
        if buf.len() < HEADER_SIZE {
            return Err(FrameHeaderError::TooShort(buf.len()));
        }

        let magic = u16::from_le_bytes([buf[0], buf[1]]);
        if magic != FRAME_MAGIC {
            return Err(FrameHeaderError::BadMagic { got: magic });
        }

        let version = buf[2];
        if version != SUPPORTED_VERSION {
            return Err(FrameHeaderError::UnsupportedVersion(version));
        }

        let pixel_format = u8_to_pixel_format(buf[3]);
        let width = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let height = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
        let seq = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);

        let header = FrameHeader {
            pixel_format,
            width,
            height,
            seq,
        };
        Ok((header, &buf[HEADER_SIZE..]))
    }
}

/// Map a [`PixelFormat`] variant to its stable 1-byte discriminant code.
pub fn pixel_format_to_u8(pf: &PixelFormat) -> u8 {
    match pf {
        PixelFormat::Unknown => 0,
        PixelFormat::Mono8 => 1,
        PixelFormat::Mono10 => 2,
        PixelFormat::Mono12 => 3,
        PixelFormat::Mono16 => 4,
        PixelFormat::BayerRG8 => 5,
        PixelFormat::BayerGR8 => 6,
        PixelFormat::BayerBG8 => 7,
        PixelFormat::BayerGB8 => 8,
        PixelFormat::BayerRG10 => 9,
        PixelFormat::BayerGR10 => 10,
        PixelFormat::BayerBG10 => 11,
        PixelFormat::BayerGB10 => 12,
        PixelFormat::BayerRG12 => 13,
        PixelFormat::BayerGR12 => 14,
        PixelFormat::BayerBG12 => 15,
        PixelFormat::BayerGB12 => 16,
        PixelFormat::BayerRG16 => 17,
        PixelFormat::BayerGR16 => 18,
        PixelFormat::BayerBG16 => 19,
        PixelFormat::BayerGB16 => 20,
        PixelFormat::RGB8 => 21,
        PixelFormat::BGR8 => 22,
        PixelFormat::RGBa8 => 23,
        PixelFormat::YCbCr422_8 => 24,
        PixelFormat::YCbCr8 => 25,
        PixelFormat::Coord3dC16 => 26,
    }
}

/// Map a 1-byte discriminant code back to the corresponding [`PixelFormat`].
pub fn u8_to_pixel_format(code: u8) -> PixelFormat {
    match code {
        0 => PixelFormat::Unknown,
        1 => PixelFormat::Mono8,
        2 => PixelFormat::Mono10,
        3 => PixelFormat::Mono12,
        4 => PixelFormat::Mono16,
        5 => PixelFormat::BayerRG8,
        6 => PixelFormat::BayerGR8,
        7 => PixelFormat::BayerBG8,
        8 => PixelFormat::BayerGB8,
        9 => PixelFormat::BayerRG10,
        10 => PixelFormat::BayerGR10,
        11 => PixelFormat::BayerBG10,
        12 => PixelFormat::BayerGB10,
        13 => PixelFormat::BayerRG12,
        14 => PixelFormat::BayerGR12,
        15 => PixelFormat::BayerBG12,
        16 => PixelFormat::BayerGB12,
        17 => PixelFormat::BayerRG16,
        18 => PixelFormat::BayerGR16,
        19 => PixelFormat::BayerBG16,
        20 => PixelFormat::BayerGB16,
        21 => PixelFormat::RGB8,
        22 => PixelFormat::BGR8,
        23 => PixelFormat::RGBa8,
        24 => PixelFormat::YCbCr422_8,
        25 => PixelFormat::YCbCr8,
        26 => PixelFormat::Coord3dC16,
        _ => PixelFormat::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_header(pf: PixelFormat) -> FrameHeader {
        FrameHeader {
            pixel_format: pf,
            width: 640,
            height: 480,
            seq: 7,
        }
    }

    #[test]
    fn test_encode_decode_roundtrip_mono8() {
        let hdr = make_header(PixelFormat::Mono8);
        let encoded = hdr.encode();
        let (decoded, remaining) = FrameHeader::decode(&encoded).expect("decode should succeed");
        assert_eq!(decoded, hdr);
        assert!(remaining.is_empty());
    }

    #[test]
    fn test_encode_produces_correct_magic() {
        let hdr = make_header(PixelFormat::Mono8);
        let encoded = hdr.encode();
        assert_eq!(encoded[0], 0x46);
        assert_eq!(encoded[1], 0x47);
    }

    #[test]
    fn test_encode_length() {
        let hdr = make_header(PixelFormat::Mono8);
        let encoded = hdr.encode();
        assert_eq!(encoded.len(), HEADER_SIZE);
    }

    #[test]
    fn test_decode_buffer_too_short() {
        let buf = [0u8; 15];
        let err = FrameHeader::decode(&buf).unwrap_err();
        assert_eq!(err, FrameHeaderError::TooShort(15));
    }

    #[test]
    fn test_decode_bad_magic() {
        let mut buf = make_header(PixelFormat::Mono8).encode();
        buf[0] = 0x00;
        buf[1] = 0x00;
        let err = FrameHeader::decode(&buf).unwrap_err();
        assert!(matches!(err, FrameHeaderError::BadMagic { got: 0x0000 }));
    }

    #[test]
    fn test_decode_unsupported_version() {
        let mut buf = make_header(PixelFormat::Mono8).encode();
        buf[2] = 2;
        let err = FrameHeader::decode(&buf).unwrap_err();
        assert_eq!(err, FrameHeaderError::UnsupportedVersion(2));
    }

    #[test]
    fn test_pixel_format_roundtrip_all_known() {
        let variants = [
            PixelFormat::Mono8,
            PixelFormat::Mono10,
            PixelFormat::Mono12,
            PixelFormat::Mono16,
            PixelFormat::BayerRG8,
            PixelFormat::BayerGR8,
            PixelFormat::BayerBG8,
            PixelFormat::BayerGB8,
            PixelFormat::BayerRG10,
            PixelFormat::BayerGR10,
            PixelFormat::BayerBG10,
            PixelFormat::BayerGB10,
            PixelFormat::BayerRG12,
            PixelFormat::BayerGR12,
            PixelFormat::BayerBG12,
            PixelFormat::BayerGB12,
            PixelFormat::BayerRG16,
            PixelFormat::BayerGR16,
            PixelFormat::BayerBG16,
            PixelFormat::BayerGB16,
            PixelFormat::RGB8,
            PixelFormat::BGR8,
            PixelFormat::RGBa8,
            PixelFormat::YCbCr422_8,
            PixelFormat::YCbCr8,
            PixelFormat::Coord3dC16,
        ];
        for pf in &variants {
            let code = pixel_format_to_u8(pf);
            let roundtripped = u8_to_pixel_format(code);
            assert_eq!(
                &roundtripped, pf,
                "roundtrip failed for {pf:?}: code={code}"
            );
        }
    }

    #[test]
    fn test_pixel_format_unknown_code() {
        assert_eq!(u8_to_pixel_format(0xFF), PixelFormat::Unknown);
    }
}
