#![cfg_attr(docsrs, feature(doc_cfg))]
//! Pixel Format Naming Convention helpers.

use core::fmt;

/// Enumeration of the pixel formats supported by the helper routines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum PixelFormat {
    Mono8 = 0x0108_0001,
    Mono16 = 0x0110_0007,
    BayerRG8 = 0x0108_0009,
    BayerGB8 = 0x0108_000A,
    BayerBG8 = 0x0108_000B,
    BayerGR8 = 0x0108_0008,
    RGB8Packed = 0x0218_0014,
    BGR8Packed = 0x0218_0015,
    /// Unknown PFNC code reported by the device.
    Unknown(u32),
}

impl PixelFormat {
    /// Convert a raw PFNC code into a [`PixelFormat`] enumeration.
    pub const fn from_code(code: u32) -> PixelFormat {
        match code {
            0x0108_0001 => PixelFormat::Mono8,
            0x0110_0007 => PixelFormat::Mono16,
            0x0108_0009 => PixelFormat::BayerRG8,
            0x0108_000A => PixelFormat::BayerGB8,
            0x0108_000B => PixelFormat::BayerBG8,
            0x0108_0008 => PixelFormat::BayerGR8,
            0x0218_0014 => PixelFormat::RGB8Packed,
            0x0218_0015 => PixelFormat::BGR8Packed,
            other => PixelFormat::Unknown(other),
        }
    }

    /// Return the PFNC code associated with the pixel format.
    pub const fn code(self) -> u32 {
        match self {
            PixelFormat::Mono8 => 0x0108_0001,
            PixelFormat::Mono16 => 0x0110_0007,
            PixelFormat::BayerRG8 => 0x0108_0009,
            PixelFormat::BayerGB8 => 0x0108_000A,
            PixelFormat::BayerBG8 => 0x0108_000B,
            PixelFormat::BayerGR8 => 0x0108_0008,
            PixelFormat::RGB8Packed => 0x0218_0014,
            PixelFormat::BGR8Packed => 0x0218_0015,
            PixelFormat::Unknown(code) => code,
        }
    }

    /// Number of bytes used to encode a single pixel for well-known formats.
    pub const fn bytes_per_pixel(self) -> Option<usize> {
        match self {
            PixelFormat::Mono8 => Some(1),
            PixelFormat::Mono16 => Some(2),
            PixelFormat::RGB8Packed | PixelFormat::BGR8Packed => Some(3),
            PixelFormat::BayerRG8
            | PixelFormat::BayerGB8
            | PixelFormat::BayerBG8
            | PixelFormat::BayerGR8 => Some(1),
            PixelFormat::Unknown(_) => None,
        }
    }

    /// Whether the pixel format represents a Bayer mosaic.
    pub const fn is_bayer(self) -> bool {
        matches!(
            self,
            PixelFormat::BayerRG8
                | PixelFormat::BayerGB8
                | PixelFormat::BayerBG8
                | PixelFormat::BayerGR8
        )
    }

    /// Return the Color Filter Array pattern and canonical offsets.
    ///
    /// The tuple encodes `(pattern, x_offset, y_offset)` where the offsets
    /// describe how the sensor mosaic aligns to the canonical `"RGGB"`
    /// ordering.
    pub const fn cfa_pattern(self) -> Option<(&'static str, u8, u8)> {
        match self {
            PixelFormat::BayerRG8 => Some(("RGGB", 0, 0)),
            PixelFormat::BayerGR8 => Some(("RGGB", 1, 0)),
            PixelFormat::BayerGB8 => Some(("RGGB", 0, 1)),
            PixelFormat::BayerBG8 => Some(("RGGB", 1, 1)),
            _ => None,
        }
    }
}

impl fmt::Display for PixelFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PixelFormat::Mono8 => f.write_str("Mono8"),
            PixelFormat::Mono16 => f.write_str("Mono16"),
            PixelFormat::BayerRG8 => f.write_str("BayerRG8"),
            PixelFormat::BayerGB8 => f.write_str("BayerGB8"),
            PixelFormat::BayerBG8 => f.write_str("BayerBG8"),
            PixelFormat::BayerGR8 => f.write_str("BayerGR8"),
            PixelFormat::RGB8Packed => f.write_str("RGB8Packed"),
            PixelFormat::BGR8Packed => f.write_str("BGR8Packed"),
            PixelFormat::Unknown(code) => write!(f, "Unknown(0x{code:08X})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PixelFormat;

    #[test]
    fn roundtrip_known_codes() {
        let formats = [
            PixelFormat::Mono8,
            PixelFormat::Mono16,
            PixelFormat::BayerRG8,
            PixelFormat::BayerGB8,
            PixelFormat::BayerBG8,
            PixelFormat::BayerGR8,
            PixelFormat::RGB8Packed,
            PixelFormat::BGR8Packed,
        ];

        for fmt in formats {
            let code = fmt.code();
            assert_eq!(PixelFormat::from_code(code), fmt);
        }
    }

    #[test]
    fn unknown_code_roundtrip() {
        let code = 0xDEAD_BEEF;
        let fmt = PixelFormat::from_code(code);
        assert!(matches!(fmt, PixelFormat::Unknown(value) if value == code));
        assert_eq!(fmt.code(), code);
    }

    #[test]
    fn bytes_per_pixel_matches_expectations() {
        assert_eq!(PixelFormat::Mono8.bytes_per_pixel(), Some(1));
        assert_eq!(PixelFormat::Mono16.bytes_per_pixel(), Some(2));
        assert_eq!(PixelFormat::RGB8Packed.bytes_per_pixel(), Some(3));
        assert_eq!(PixelFormat::BayerRG8.bytes_per_pixel(), Some(1));
        assert_eq!(PixelFormat::Unknown(0).bytes_per_pixel(), None);
    }

    #[test]
    fn cfa_offsets_align_to_rggb() {
        assert_eq!(PixelFormat::BayerRG8.cfa_pattern(), Some(("RGGB", 0, 0)));
        assert_eq!(PixelFormat::BayerGR8.cfa_pattern(), Some(("RGGB", 1, 0)));
        assert_eq!(PixelFormat::BayerGB8.cfa_pattern(), Some(("RGGB", 0, 1)));
        assert_eq!(PixelFormat::BayerBG8.cfa_pattern(), Some(("RGGB", 1, 1)));
        assert_eq!(PixelFormat::Mono8.cfa_pattern(), None);
    }
}
