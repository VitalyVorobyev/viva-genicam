//! Binary image envelope on the Zenoh image key.
//!
//! Version 3 keeps the version-2 geometry offsets, then carries a bounded
//! JSON metadata object in the same packet as its pixels. The fixed prefix
//! is 24 bytes, all integers little endian:
//!
//! | Offset | Size | Field |
//! |--------|------|-------|
//! | 0 | 2 | magic `0x4746` |
//! | 2 | 1 | version `3` |
//! | 3 | 1 | pixel format code |
//! | 4 | 4 | width |
//! | 8 | 4 | height |
//! | 12 | 4 | sequence counter |
//! | 16 | 4 | row stride, or 0 for continuous packing |
//! | 20 | 4 | metadata JSON byte length, at most 64 KiB |
//!
//! Metadata bytes follow, then the image pixels. Length 0 means no metadata.

use crate::{FrameMetadata, MAX_FRAME_METADATA_BYTES};
use thiserror::Error;
use viva_pfnc::PixelFormat;

/// Little-endian image magic, bytes `[0x46, 0x47]`.
pub const FRAME_MAGIC: u16 = 0x4746;
/// Fixed prefix size, excluding the optional metadata object and pixels.
pub const HEADER_SIZE: usize = 24;
/// The only supported image envelope version.
pub const SUPPORTED_VERSION: u8 = 3;

/// A malformed or unrepresentable image envelope.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum FrameHeaderError {
    /// The fixed prefix is incomplete.
    #[error("buffer too short: need {HEADER_SIZE} bytes, got {0}")]
    TooShort(usize),
    /// The image magic is wrong.
    #[error("bad magic: expected 0x{:04X}, got 0x{got:04X}", FRAME_MAGIC)]
    BadMagic {
        /// What the packet's magic field holds.
        got: u16,
    },
    /// The sender uses a different envelope version.
    #[error("unsupported version {0}; only version {SUPPORTED_VERSION} is supported")]
    UnsupportedVersion(u8),
    /// Metadata exceeds the packet's bounded allocation limit.
    #[error("frame metadata length {0} exceeds {MAX_FRAME_METADATA_BYTES}")]
    MetadataTooLarge(usize),
    /// The declared metadata length exceeds the available bytes.
    #[error("frame metadata is truncated")]
    TruncatedMetadata,
    /// Metadata JSON or its field bounds are invalid.
    #[error("invalid frame metadata: {0}")]
    InvalidMetadata(String),
}

/// Geometry and optional metadata for the pixels that follow it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameHeader {
    /// PFNC format projected onto the wire's byte code.
    pub pixel_format: PixelFormat,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Service image sequence, wrapping at `u32::MAX`.
    pub seq: u32,
    /// Bytes per padded row; `None` means continuous packing.
    pub row_stride: Option<u32>,
    /// Facts from this exact frame, inline rather than separately published.
    pub metadata: Option<FrameMetadata>,
}

impl FrameHeader {
    /// Encode the prefix and bounded metadata. Append the pixels afterward.
    pub fn encode(&self) -> Result<Vec<u8>, FrameHeaderError> {
        let metadata = self
            .metadata
            .as_ref()
            .map(FrameMetadata::encode)
            .transpose()
            .map_err(|e| FrameHeaderError::InvalidMetadata(e.to_string()))?
            .unwrap_or_default();
        let mut buf = Vec::with_capacity(HEADER_SIZE + metadata.len());
        buf.extend_from_slice(&FRAME_MAGIC.to_le_bytes());
        buf.push(SUPPORTED_VERSION);
        buf.push(pixel_format_to_u8(self.pixel_format));
        buf.extend_from_slice(&self.width.to_le_bytes());
        buf.extend_from_slice(&self.height.to_le_bytes());
        buf.extend_from_slice(&self.seq.to_le_bytes());
        buf.extend_from_slice(&self.row_stride.unwrap_or(0).to_le_bytes());
        buf.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
        buf.extend_from_slice(&metadata);
        Ok(buf)
    }

    /// Decode bounded metadata before exposing the remaining image bytes.
    /// Older envelope versions are rejected, never read with guessed offsets.
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
        let size = u32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]) as usize;
        if size > MAX_FRAME_METADATA_BYTES {
            return Err(FrameHeaderError::MetadataTooLarge(size));
        }
        let end = HEADER_SIZE
            .checked_add(size)
            .filter(|&end| end <= buf.len())
            .ok_or(FrameHeaderError::TruncatedMetadata)?;
        let metadata = if size == 0 {
            None
        } else {
            Some(
                FrameMetadata::decode(&buf[HEADER_SIZE..end])
                    .map_err(FrameHeaderError::InvalidMetadata)?,
            )
        };
        let stride = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]);
        Ok((
            Self {
                pixel_format: u8_to_pixel_format(buf[3]),
                width: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
                height: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
                seq: u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]),
                row_stride: (stride != 0).then_some(stride),
                metadata,
            },
            &buf[end..],
        ))
    }
}

/// The frame header's one-byte format codes.
///
/// The Zenoh wire's own map, and nothing more: what a format *is* is
/// `viva-pfnc`'s to say (ADR-0030); this table says which byte carries it.
/// Codes are only appended, never reordered. A format without a code travels
/// as 0 and decodes as `PixelFormat::Unknown(0)`; [`ImageMeta`](crate::ImageMeta)
/// names it in full. Giving `Coord3D_*` codes (backlog DC-02) changes the wire.
const WIRE_CODES: &[(u8, PixelFormat)] = &[
    (1, PixelFormat::Mono8),
    (2, PixelFormat::Mono10),
    (3, PixelFormat::Mono12),
    (4, PixelFormat::Mono16),
    (5, PixelFormat::BayerRG8),
    (6, PixelFormat::BayerGR8),
    (7, PixelFormat::BayerBG8),
    (8, PixelFormat::BayerGB8),
    (9, PixelFormat::BayerRG10),
    (10, PixelFormat::BayerGR10),
    (11, PixelFormat::BayerBG10),
    (12, PixelFormat::BayerGB10),
    (13, PixelFormat::BayerRG12),
    (14, PixelFormat::BayerGR12),
    (15, PixelFormat::BayerBG12),
    (16, PixelFormat::BayerGB12),
    (17, PixelFormat::BayerRG16),
    (18, PixelFormat::BayerGR16),
    (19, PixelFormat::BayerBG16),
    (20, PixelFormat::BayerGB16),
    (21, PixelFormat::RGB8Packed),
    (22, PixelFormat::BGR8Packed),
    (23, PixelFormat::RGBa8),
    (24, PixelFormat::YCbCr422_8),
    (25, PixelFormat::YCbCr8),
    (26, PixelFormat::Coord3DC16),
    (27, PixelFormat::Mono14),
    (28, PixelFormat::Mono10p),
    (29, PixelFormat::Mono12p),
    (30, PixelFormat::Mono10Packed),
    (31, PixelFormat::Mono12Packed),
    (32, PixelFormat::BayerRG10p),
    (33, PixelFormat::BayerGR10p),
    (34, PixelFormat::BayerBG10p),
    (35, PixelFormat::BayerGB10p),
    (36, PixelFormat::BayerRG12p),
    (37, PixelFormat::BayerGR12p),
    (38, PixelFormat::BayerBG12p),
    (39, PixelFormat::BayerGB12p),
    (40, PixelFormat::BayerRG10Packed),
    (41, PixelFormat::BayerGR10Packed),
    (42, PixelFormat::BayerBG10Packed),
    (43, PixelFormat::BayerGB10Packed),
    (44, PixelFormat::BayerRG12Packed),
    (45, PixelFormat::BayerGR12Packed),
    (46, PixelFormat::BayerBG12Packed),
    (47, PixelFormat::BayerGB12Packed),
    (48, PixelFormat::BGRa8),
    (49, PixelFormat::YUV422_8),
    (50, PixelFormat::YUV422_8_UYVY),
    (51, PixelFormat::YCbCr422_8_CbYCrY),
    (52, PixelFormat::YUV411_8_UYYVYY),
    (53, PixelFormat::YUV8_UYV),
    (54, PixelFormat::YCbCr8_CbYCr),
];

/// The one-byte code the frame header carries `format` as; 0 for a format
/// without one.
pub fn pixel_format_to_u8(format: PixelFormat) -> u8 {
    WIRE_CODES
        .iter()
        .find(|&&(_, named)| named == format)
        .map_or(0, |&(code, _)| code)
}

/// The format a frame header's one-byte code names; `PixelFormat::Unknown(0)`
/// for 0 and for a code this table does not know.
pub fn u8_to_pixel_format(code: u8) -> PixelFormat {
    WIRE_CODES
        .iter()
        .find(|&&(known, _)| known == code)
        .map_or(PixelFormat::Unknown(0), |&(_, format)| format)
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
            row_stride: None,
            metadata: None,
        }
    }

    #[test]
    fn v3_wire_layout_has_literal_offsets_and_continuous_sentinel() {
        let header = FrameHeader {
            pixel_format: PixelFormat::Mono8,
            width: 0x12345678,
            height: 2,
            seq: 0x44332211,
            row_stride: Some(0x88776655),
            metadata: None,
        };
        let expected = [
            0x46, 0x47, 3, 1, 0x78, 0x56, 0x34, 0x12, 2, 0, 0, 0, 0x11, 0x22, 0x33, 0x44, 0x55,
            0x66, 0x77, 0x88, 0, 0, 0, 0,
        ];
        assert_eq!(header.encode().unwrap(), expected);
        let mut wire = expected.to_vec();
        wire.extend_from_slice(&[9, 8]);
        assert_eq!(FrameHeader::decode(&wire).unwrap(), (header, &[9, 8][..]));
        wire[16..20].fill(0);
        assert_eq!(FrameHeader::decode(&wire).unwrap().0.row_stride, None);
        for version in [0, 1, 2, 255] {
            wire[2] = version;
            assert_eq!(
                FrameHeader::decode(&wire),
                Err(FrameHeaderError::UnsupportedVersion(version))
            );
        }
        for length in 0..24 {
            assert_eq!(
                FrameHeader::decode(&wire[..length]),
                Err(FrameHeaderError::TooShort(length))
            );
        }
    }

    #[test]
    fn test_encode_decode_roundtrip_mono8() {
        let hdr = make_header(PixelFormat::Mono8);
        let encoded = hdr.encode().unwrap();
        let (decoded, remaining) = FrameHeader::decode(&encoded).expect("decode should succeed");
        assert_eq!(decoded, hdr);
        assert!(remaining.is_empty());
    }

    #[test]
    fn test_encode_produces_correct_magic() {
        let hdr = make_header(PixelFormat::Mono8);
        let encoded = hdr.encode().unwrap();
        assert_eq!(encoded[0], 0x46);
        assert_eq!(encoded[1], 0x47);
    }

    #[test]
    fn test_encode_length() {
        let hdr = make_header(PixelFormat::Mono8);
        let encoded = hdr.encode().unwrap();
        assert_eq!(encoded.len(), HEADER_SIZE);
    }

    #[test]
    fn test_decode_buffer_too_short() {
        let buf = [0u8; 19];
        let err = FrameHeader::decode(&buf).unwrap_err();
        assert_eq!(err, FrameHeaderError::TooShort(19));
    }

    #[test]
    fn test_decode_bad_magic() {
        let mut buf = make_header(PixelFormat::Mono8).encode().unwrap();
        buf[0] = 0x00;
        buf[1] = 0x00;
        let err = FrameHeader::decode(&buf).unwrap_err();
        assert!(matches!(err, FrameHeaderError::BadMagic { got: 0x0000 }));
    }

    #[test]
    fn test_decode_unsupported_version() {
        let mut buf = make_header(PixelFormat::Mono8).encode().unwrap();
        buf[2] = 1;
        let err = FrameHeader::decode(&buf).unwrap_err();
        assert_eq!(err, FrameHeaderError::UnsupportedVersion(1));
    }

    /// The published codes keep their meaning, written out as literals
    /// rather than from the table: a client built before the table moved
    /// onto `viva-pfnc` reads the same bytes.
    #[test]
    fn the_published_codes_keep_their_meaning() {
        let published: &[(u8, &str)] = &[
            (1, "Mono8"),
            (2, "Mono10"),
            (3, "Mono12"),
            (4, "Mono16"),
            (5, "BayerRG8"),
            (20, "BayerGB16"),
            (21, "RGB8Packed"),
            (22, "BGR8Packed"),
            (23, "RGBa8"),
            (24, "YCbCr422_8"),
            (25, "YCbCr8"),
            (26, "Coord3D_C16"),
            (27, "Mono14"),
            (28, "Mono10p"),
            (31, "Mono12Packed"),
            (48, "BGRa8"),
            (50, "YUV422_8_UYVY"),
            (54, "YCbCr8_CbYCr"),
        ];
        for &(code, name) in published {
            let format = PixelFormat::from_name(name);
            assert_eq!(pixel_format_to_u8(format), code, "{name}");
            assert_eq!(u8_to_pixel_format(code), format, "{code}");
        }
    }

    /// Every code names one format, and every coded format round-trips.
    #[test]
    fn every_code_is_its_own_format() {
        let mut seen = std::collections::HashSet::new();
        for &(code, format) in WIRE_CODES {
            assert_ne!(code, 0, "{format}");
            assert!(seen.insert(code), "{format} reuses code {code}");
            assert!(
                !matches!(format, PixelFormat::Unknown(_)),
                "code {code} names no PFNC format"
            );
            assert_eq!(u8_to_pixel_format(pixel_format_to_u8(format)), format);
        }
    }

    /// A format without a code travels as 0, and 0 and unknown codes decode
    /// as unknown.
    #[test]
    fn a_format_without_a_code_is_unknown_on_the_wire() {
        assert_eq!(pixel_format_to_u8(PixelFormat::Coord3DABC32f), 0);
        assert_eq!(u8_to_pixel_format(0), PixelFormat::Unknown(0));
        assert_eq!(u8_to_pixel_format(0xFF), PixelFormat::Unknown(0));
    }
}

#[cfg(test)]
mod metadata_tests {
    use super::*;

    #[test]
    fn inline_metadata_stays_with_its_pixels() {
        let header = FrameHeader {
            pixel_format: PixelFormat::Mono8,
            width: 2,
            height: 1,
            seq: 9,
            row_stride: None,
            metadata: Some(FrameMetadata {
                block_id: u64::MAX.to_string(),
                ts_dev: Some(u64::MAX.to_string()),
                ts_host_ns: None,
                complete: true,
                chunks: Vec::new(),
                features: Vec::new(),
                chunk_error: None,
                truncated: false,
            }),
        };
        let mut packet = header.encode().unwrap();
        packet.extend_from_slice(&[21, 22]);
        let (decoded, pixels) = FrameHeader::decode(&packet).unwrap();
        assert_eq!(decoded, header);
        assert_eq!(pixels, &[21, 22]);
    }

    #[test]
    fn malformed_and_oversized_metadata_never_becomes_pixels() {
        let header = FrameHeader {
            pixel_format: PixelFormat::Mono8,
            width: 1,
            height: 1,
            seq: 0,
            row_stride: None,
            metadata: None,
        };
        let mut packet = header.encode().unwrap();
        packet[20..24].copy_from_slice(&((MAX_FRAME_METADATA_BYTES + 1) as u32).to_le_bytes());
        assert_eq!(
            FrameHeader::decode(&packet),
            Err(FrameHeaderError::MetadataTooLarge(
                MAX_FRAME_METADATA_BYTES + 1
            ))
        );
        packet[20..24].copy_from_slice(&8u32.to_le_bytes());
        assert_eq!(
            FrameHeader::decode(&packet),
            Err(FrameHeaderError::TruncatedMetadata)
        );
        packet[20..24].copy_from_slice(&1u32.to_le_bytes());
        packet.push(b'{');
        assert!(matches!(
            FrameHeader::decode(&packet),
            Err(FrameHeaderError::InvalidMetadata(_))
        ));
    }
}
