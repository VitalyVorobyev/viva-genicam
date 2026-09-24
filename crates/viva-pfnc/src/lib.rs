#![cfg_attr(docsrs, feature(doc_cfg))]
//! Pixel Format Naming Convention helpers.

use core::fmt;

/// Enumeration of the pixel formats supported by the helper routines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
#[repr(u32)]
pub enum PixelFormat {
    Mono8 = 0x0108_0001,
    Mono10 = 0x0110_0003,
    Mono12 = 0x0110_0005,
    Mono14 = 0x0110_0025,
    Mono16 = 0x0110_0007,
    Confidence8 = 0x0108_00C6,
    Coord3DC32f = 0x0120_00BF,
    Coord3DAC16 = 0x0220_00BB,
    Coord3DAC32f = 0x0240_00C2,
    Coord3DABC32f = 0x0260_00C0,
    BayerRG8 = 0x0108_0009,
    BayerGB8 = 0x0108_000A,
    BayerBG8 = 0x0108_000B,
    BayerGR8 = 0x0108_0008,
    BayerGR16 = 0x0110_000E,
    BayerRG16 = 0x0110_000F,
    BayerGB16 = 0x0110_0010,
    BayerBG16 = 0x0110_0011,
    RGB8Packed = 0x0218_0014,
    BGR8Packed = 0x0218_0015,
    /// Prophesee EVT 3.0 event stream, as a Lucid Triton2 EVS sends it
    /// ([#139](https://github.com/VitalyVorobyev/viva-genicam/pull/139)).
    ///
    /// **Vendor-defined, not PFNC**: bit 31 of the code is set, which is how
    /// PFNC marks a format outside its own table. The stream carries events,
    /// not pixels, so [`PixelFormat::bytes_per_pixel`] answers the size of one
    /// event *word* (16 bits) — the depth the code's own bits 23-16 declare,
    /// and the same answer [`PixelFormat::Unknown`] would derive for it. A
    /// block's length is not `width * height` times that.
    EvsEvt30 = 0x8110_0E30,
    /// Prophesee EVT 2.1 event stream, 64-bit event words. Vendor-defined; see
    /// [`PixelFormat::EvsEvt30`].
    EvsEvt21 = 0x8140_0E21,
    /// Unknown PFNC code reported by the device.
    Unknown(u32),
}

impl PixelFormat {
    /// Convert a raw PFNC code into a [`PixelFormat`] enumeration.
    pub const fn from_code(code: u32) -> PixelFormat {
        match code {
            0x0108_0001 => PixelFormat::Mono8,
            0x0110_0003 => PixelFormat::Mono10,
            0x0110_0005 => PixelFormat::Mono12,
            0x0110_0025 => PixelFormat::Mono14,
            0x0110_0007 => PixelFormat::Mono16,
            0x0108_00C6 => PixelFormat::Confidence8,
            0x0120_00BF => PixelFormat::Coord3DC32f,
            0x0220_00BB => PixelFormat::Coord3DAC16,
            0x0240_00C2 => PixelFormat::Coord3DAC32f,
            0x0260_00C0 => PixelFormat::Coord3DABC32f,
            0x0108_0009 => PixelFormat::BayerRG8,
            0x0108_000A => PixelFormat::BayerGB8,
            0x0108_000B => PixelFormat::BayerBG8,
            0x0108_0008 => PixelFormat::BayerGR8,
            0x0110_000E => PixelFormat::BayerGR16,
            0x0110_000F => PixelFormat::BayerRG16,
            0x0110_0010 => PixelFormat::BayerGB16,
            0x0110_0011 => PixelFormat::BayerBG16,
            0x0218_0014 => PixelFormat::RGB8Packed,
            0x0218_0015 => PixelFormat::BGR8Packed,
            0x8110_0E30 => PixelFormat::EvsEvt30,
            0x8140_0E21 => PixelFormat::EvsEvt21,
            other => PixelFormat::Unknown(other),
        }
    }

    /// Return the PFNC code associated with the pixel format.
    pub const fn code(self) -> u32 {
        match self {
            PixelFormat::Mono8 => 0x0108_0001,
            PixelFormat::Mono10 => 0x0110_0003,
            PixelFormat::Mono12 => 0x0110_0005,
            PixelFormat::Mono14 => 0x0110_0025,
            PixelFormat::Mono16 => 0x0110_0007,
            PixelFormat::Confidence8 => 0x0108_00C6,
            PixelFormat::Coord3DC32f => 0x0120_00BF,
            PixelFormat::Coord3DAC16 => 0x0220_00BB,
            PixelFormat::Coord3DAC32f => 0x0240_00C2,
            PixelFormat::Coord3DABC32f => 0x0260_00C0,
            PixelFormat::BayerRG8 => 0x0108_0009,
            PixelFormat::BayerGB8 => 0x0108_000A,
            PixelFormat::BayerBG8 => 0x0108_000B,
            PixelFormat::BayerGR8 => 0x0108_0008,
            PixelFormat::BayerGR16 => 0x0110_000E,
            PixelFormat::BayerRG16 => 0x0110_000F,
            PixelFormat::BayerGB16 => 0x0110_0010,
            PixelFormat::BayerBG16 => 0x0110_0011,
            PixelFormat::RGB8Packed => 0x0218_0014,
            PixelFormat::BGR8Packed => 0x0218_0015,
            PixelFormat::EvsEvt30 => 0x8110_0E30,
            PixelFormat::EvsEvt21 => 0x8140_0E21,
            PixelFormat::Unknown(code) => code,
        }
    }

    /// Number of bytes used to encode a single pixel.
    ///
    /// For a format this enumeration does not name, the answer is derived from
    /// bits 23-16 of the PFNC code, which carry the pixel's bit depth. `None`
    /// means the pixel genuinely has no whole-byte size — a packed format — not
    /// that we failed to look it up.
    pub const fn bytes_per_pixel(self) -> Option<usize> {
        match self {
            PixelFormat::Mono8 => Some(1),
            PixelFormat::Mono10 | PixelFormat::Mono12 | PixelFormat::Mono14 => Some(2),
            PixelFormat::Mono16 => Some(2),
            PixelFormat::Confidence8 => Some(1),
            PixelFormat::Coord3DC32f => Some(4),
            PixelFormat::Coord3DAC16 => Some(4),
            PixelFormat::Coord3DAC32f => Some(8),
            PixelFormat::Coord3DABC32f => Some(12),
            PixelFormat::RGB8Packed | PixelFormat::BGR8Packed => Some(3),
            PixelFormat::BayerRG8
            | PixelFormat::BayerGB8
            | PixelFormat::BayerBG8
            | PixelFormat::BayerGR8 => Some(1),
            PixelFormat::BayerGR16
            | PixelFormat::BayerRG16
            | PixelFormat::BayerGB16
            | PixelFormat::BayerBG16 => Some(2),
            PixelFormat::EvsEvt30 => Some(2),
            PixelFormat::EvsEvt21 => Some(8),
            PixelFormat::Unknown(code) => PixelFormat::bytes_from_code(code),
        }
    }

    /// Bytes per pixel read straight out of a PFNC code.
    ///
    /// Bits 23-16 of every PFNC value are the pixel's bit depth, so a format
    /// this enumeration has no variant for still has a usable size. That is the
    /// difference between a receiver sizing a `Coord3D_ABC32f` frame at twelve
    /// bytes per pixel and sizing it at one: callers overwhelmingly write
    /// `bytes_per_pixel().unwrap_or(1)`, and a `None` there is not a neutral
    /// answer, it is a wrong one.
    ///
    /// Returns `None` when the depth is not a whole number of bytes, because
    /// then no `usize` is correct. **Packed formats are the reason this check
    /// exists**: `Mono12Packed`, `Mono10Packed`, `YUV411Packed`,
    /// `BayerGR12Packed` and `BayerRG12Packed` all declare 12 bits, and eleven
    /// of the 37 vendor-corpus documents offer at least one of them. Rounding
    /// 12 bits up to 2 bytes overstates a frame by a third, which a length
    /// check downstream reads as a *short payload* — a confidently wrong size
    /// is worse than an absent one.
    const fn bytes_from_code(code: u32) -> Option<usize> {
        let bits = (code >> 16) & 0xFF;
        if bits == 0 || !bits.is_multiple_of(8) {
            return None;
        }
        Some((bits / 8) as usize)
    }

    /// Convert a PFNC name string to a [`PixelFormat`].
    ///
    /// Returns `PixelFormat::Unknown(0)` for unrecognised names.
    pub fn from_name(name: &str) -> PixelFormat {
        match name {
            "Mono8" => PixelFormat::Mono8,
            "Mono10" => PixelFormat::Mono10,
            "Mono12" => PixelFormat::Mono12,
            "Mono14" => PixelFormat::Mono14,
            "Mono16" => PixelFormat::Mono16,
            "Confidence8" => PixelFormat::Confidence8,
            "Coord3D_C32f" => PixelFormat::Coord3DC32f,
            "Coord3D_AC16" => PixelFormat::Coord3DAC16,
            "Coord3D_AC32f" => PixelFormat::Coord3DAC32f,
            "Coord3D_ABC32f" => PixelFormat::Coord3DABC32f,
            "BayerRG8" => PixelFormat::BayerRG8,
            "BayerGB8" => PixelFormat::BayerGB8,
            "BayerBG8" => PixelFormat::BayerBG8,
            "BayerGR8" => PixelFormat::BayerGR8,
            "BayerGR16" => PixelFormat::BayerGR16,
            "BayerRG16" => PixelFormat::BayerRG16,
            "BayerGB16" => PixelFormat::BayerGB16,
            "BayerBG16" => PixelFormat::BayerBG16,
            "RGB8Packed" | "RGB8" => PixelFormat::RGB8Packed,
            "BGR8Packed" | "BGR8" => PixelFormat::BGR8Packed,
            "EVT3_0" => PixelFormat::EvsEvt30,
            "EVT2_1" => PixelFormat::EvsEvt21,
            _ => PixelFormat::Unknown(0),
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
                | PixelFormat::BayerGR16
                | PixelFormat::BayerRG16
                | PixelFormat::BayerGB16
                | PixelFormat::BayerBG16
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
            PixelFormat::Mono10 => f.write_str("Mono10"),
            PixelFormat::Mono12 => f.write_str("Mono12"),
            PixelFormat::Mono14 => f.write_str("Mono14"),
            PixelFormat::Mono16 => f.write_str("Mono16"),
            PixelFormat::Confidence8 => f.write_str("Confidence8"),
            PixelFormat::Coord3DC32f => f.write_str("Coord3D_C32f"),
            PixelFormat::Coord3DAC16 => f.write_str("Coord3D_AC16"),
            PixelFormat::Coord3DAC32f => f.write_str("Coord3D_AC32f"),
            PixelFormat::Coord3DABC32f => f.write_str("Coord3D_ABC32f"),
            PixelFormat::BayerRG8 => f.write_str("BayerRG8"),
            PixelFormat::BayerGB8 => f.write_str("BayerGB8"),
            PixelFormat::BayerBG8 => f.write_str("BayerBG8"),
            PixelFormat::BayerGR8 => f.write_str("BayerGR8"),
            PixelFormat::BayerGR16 => f.write_str("BayerGR16"),
            PixelFormat::BayerRG16 => f.write_str("BayerRG16"),
            PixelFormat::BayerGB16 => f.write_str("BayerGB16"),
            PixelFormat::BayerBG16 => f.write_str("BayerBG16"),
            PixelFormat::RGB8Packed => f.write_str("RGB8Packed"),
            PixelFormat::BGR8Packed => f.write_str("BGR8Packed"),
            PixelFormat::EvsEvt30 => f.write_str("EVT3_0"),
            PixelFormat::EvsEvt21 => f.write_str("EVT2_1"),
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
            PixelFormat::Mono10,
            PixelFormat::Mono12,
            PixelFormat::Mono14,
            PixelFormat::Mono16,
            PixelFormat::Confidence8,
            PixelFormat::Coord3DC32f,
            PixelFormat::Coord3DAC16,
            PixelFormat::Coord3DAC32f,
            PixelFormat::Coord3DABC32f,
            PixelFormat::BayerRG8,
            PixelFormat::BayerGB8,
            PixelFormat::BayerBG8,
            PixelFormat::BayerGR8,
            PixelFormat::BayerGR16,
            PixelFormat::BayerRG16,
            PixelFormat::BayerGB16,
            PixelFormat::BayerBG16,
            PixelFormat::RGB8Packed,
            PixelFormat::BGR8Packed,
            PixelFormat::EvsEvt30,
            PixelFormat::EvsEvt21,
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
        assert_eq!(PixelFormat::Mono10.bytes_per_pixel(), Some(2));
        assert_eq!(PixelFormat::Mono16.bytes_per_pixel(), Some(2));
        assert_eq!(PixelFormat::Confidence8.bytes_per_pixel(), Some(1));
        assert_eq!(PixelFormat::Coord3DC32f.bytes_per_pixel(), Some(4));
        assert_eq!(PixelFormat::Coord3DAC16.bytes_per_pixel(), Some(4));
        assert_eq!(PixelFormat::Coord3DAC32f.bytes_per_pixel(), Some(8));
        assert_eq!(PixelFormat::Coord3DABC32f.bytes_per_pixel(), Some(12));
        assert_eq!(PixelFormat::RGB8Packed.bytes_per_pixel(), Some(3));
        assert_eq!(PixelFormat::BayerRG8.bytes_per_pixel(), Some(1));
        assert_eq!(PixelFormat::BayerRG16.bytes_per_pixel(), Some(2));
        assert_eq!(PixelFormat::BayerGR16.bytes_per_pixel(), Some(2));
        assert_eq!(PixelFormat::EvsEvt30.bytes_per_pixel(), Some(2));
        assert_eq!(PixelFormat::EvsEvt21.bytes_per_pixel(), Some(8));
        assert_eq!(PixelFormat::Unknown(0).bytes_per_pixel(), None);
    }

    #[test]
    fn event_format_names_roundtrip() {
        for (name, format) in [
            ("EVT3_0", PixelFormat::EvsEvt30),
            ("EVT2_1", PixelFormat::EvsEvt21),
        ] {
            assert_eq!(PixelFormat::from_name(name), format);
            assert_eq!(format.to_string(), name);
        }
    }

    /// Every named format must agree with the size encoded in its own code,
    /// which is what makes the `Unknown` derivation trustworthy.
    #[test]
    fn named_formats_agree_with_their_own_pfnc_code() {
        let formats = [
            PixelFormat::Mono8,
            PixelFormat::Mono10,
            PixelFormat::Mono12,
            PixelFormat::Mono14,
            PixelFormat::Mono16,
            PixelFormat::Confidence8,
            PixelFormat::Coord3DC32f,
            PixelFormat::Coord3DAC16,
            PixelFormat::Coord3DAC32f,
            PixelFormat::Coord3DABC32f,
            PixelFormat::BayerRG8,
            PixelFormat::BayerGB8,
            PixelFormat::BayerBG8,
            PixelFormat::BayerGR8,
            PixelFormat::BayerGR16,
            PixelFormat::BayerRG16,
            PixelFormat::BayerGB16,
            PixelFormat::BayerBG16,
            PixelFormat::RGB8Packed,
            PixelFormat::BGR8Packed,
            PixelFormat::EvsEvt30,
            PixelFormat::EvsEvt21,
        ];

        for fmt in formats {
            // Keep the size nibble, replace the unique ID with one no format
            // uses, so `from_code` yields `Unknown` and the size has to be
            // derived rather than looked up.
            let disguised = PixelFormat::from_code((fmt.code() & 0xFFFF_0000) | 0xFFFF);
            assert!(
                matches!(disguised, PixelFormat::Unknown(_)),
                "{fmt}: the disguise resolved to a named format"
            );
            assert_eq!(
                fmt.bytes_per_pixel(),
                disguised.bytes_per_pixel(),
                "{fmt}: the hardcoded size disagrees with bits 23-16 of its own code"
            );
        }
    }

    /// A format we have no variant for still gets a size, so callers stop
    /// falling back to one byte per pixel.
    #[test]
    fn unknown_formats_are_sized_from_their_code() {
        // Real PFNC codes we deliberately do not enumerate.
        // RGBa8: 32 bits. RGB16: 48 bits. Coord3D_ABC32: 96 bits.
        assert_eq!(
            PixelFormat::from_code(0x0220_0016).bytes_per_pixel(),
            Some(4)
        );
        assert_eq!(
            PixelFormat::from_code(0x0230_0033).bytes_per_pixel(),
            Some(6)
        );
        assert_eq!(
            PixelFormat::from_code(0x0260_00C1).bytes_per_pixel(),
            Some(12)
        );
    }

    /// Packed formats have a fractional byte size, and no `usize` is right.
    ///
    /// These five all appear in the vendor XML corpus — `Mono12Packed` in
    /// eleven of its 37 documents. Rounding 12 bits up to 2 bytes would
    /// overstate a frame by a third and make a length check downstream reject
    /// it as short.
    #[test]
    fn packed_formats_report_no_whole_byte_size() {
        for (name, code) in [
            ("Mono10Packed", 0x010C_0004_u32),
            ("Mono12Packed", 0x010C_0006),
            ("YUV411Packed", 0x020C_001E),
            ("BayerGR12Packed", 0x010C_002C),
            ("BayerRG12Packed", 0x010C_002D),
        ] {
            assert_eq!(
                PixelFormat::from_code(code).bytes_per_pixel(),
                None,
                "{name} declares 12 bits per pixel and must not be rounded"
            );
        }
    }

    #[test]
    fn scancontrol_names_resolve_to_known_formats() {
        let formats = [
            ("Mono10", PixelFormat::Mono10),
            ("Confidence8", PixelFormat::Confidence8),
            ("Coord3D_C32f", PixelFormat::Coord3DC32f),
            ("Coord3D_AC16", PixelFormat::Coord3DAC16),
            ("Coord3D_AC32f", PixelFormat::Coord3DAC32f),
            ("Coord3D_ABC32f", PixelFormat::Coord3DABC32f),
        ];

        for (name, format) in formats {
            assert_eq!(PixelFormat::from_name(name), format);
            assert_eq!(format.to_string(), name);
        }
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
