#![cfg_attr(docsrs, feature(doc_cfg))]
//! Pixel Format Naming Convention (PFNC) tables, sizes and conversions.
//!
//! [`PixelFormat`] names the formats a camera reports, with the 32-bit codes
//! of EMVA's *GenICam Pixel Format Names and Values* table, and answers how
//! many bits and bytes they occupy. [`convert`] turns their bytes into
//! samples, grey or RGB. This crate is the one place pixel layouts live: the
//! library's `Frame::to_rgb8`, its image writer and Viva Studio's viewer all
//! call [`convert`].
//!
//! The crate does no I/O and has no required dependencies, and it compiles
//! for `wasm32-unknown-unknown`.

use core::fmt;

pub mod convert;

/// Declare the enumeration and its tables from one list, so a name, a code
/// and the text a format prints as cannot drift apart.
macro_rules! pixel_formats {
    ($(
        $(#[$meta:meta])*
        $variant:ident = $code:literal, $name:literal $(| $alias:literal)*;
    )*) => {
        /// Enumeration of the pixel formats supported by the helper routines.
        ///
        /// Codes are those of EMVA's *GenICam Pixel Format Names and Values*
        /// (7 October 2025), the table PFNC 2.4 Appendix C refers to.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
        #[non_exhaustive]
        #[repr(u32)]
        // Y'CbCr variants keep their PFNC spelling (`YUV422_8_UYVY`): the
        // component order after the underscore is part of the name.
        #[allow(non_camel_case_types)]
        pub enum PixelFormat {
            $($(#[$meta])* $variant = $code,)*
            /// Unknown PFNC code reported by the device.
            Unknown(u32),
        }

        impl PixelFormat {
            /// Convert a raw PFNC code into a [`PixelFormat`] enumeration.
            pub const fn from_code(code: u32) -> PixelFormat {
                match code {
                    $($code => PixelFormat::$variant,)*
                    other => PixelFormat::Unknown(other),
                }
            }

            /// Return the PFNC code associated with the pixel format.
            pub const fn code(self) -> u32 {
                match self {
                    $(PixelFormat::$variant => $code,)*
                    PixelFormat::Unknown(code) => code,
                }
            }

            /// Convert a pixel format name to a [`PixelFormat`].
            ///
            /// Accepts the PFNC name and the older spellings cameras still
            /// report for the same code — `YUV422Packed` for `YUV422_8_UYVY`,
            /// `RGB8` for `RGB8Packed`. Returns `PixelFormat::Unknown(0)` for
            /// unrecognised names.
            pub fn from_name(name: &str) -> PixelFormat {
                match name {
                    $($name $(| $alias)* => PixelFormat::$variant,)*
                    _ => PixelFormat::Unknown(0),
                }
            }

            /// The name the format prints as, `None` for an unknown code.
            const fn name(self) -> Option<&'static str> {
                match self {
                    $(PixelFormat::$variant => Some($name),)*
                    PixelFormat::Unknown(_) => None,
                }
            }

            /// Every named format, for tests that must cover them all.
            #[cfg(test)]
            const ALL: &'static [PixelFormat] = &[$(PixelFormat::$variant,)*];
        }
    };
}

pixel_formats! {
    Mono8 = 0x0108_0001, "Mono8";
    Mono10 = 0x0110_0003, "Mono10";
    Mono12 = 0x0110_0005, "Mono12";
    Mono14 = 0x0110_0025, "Mono14";
    Mono16 = 0x0110_0007, "Mono16";
    /// 10-bit monochrome, PFNC lsb packed: four pixels in five bytes, filled
    /// from bit 0 of byte 0 (PFNC 2.4 §6.3.1, Figure 6-9).
    Mono10p = 0x010A_0046, "Mono10p";
    /// 12-bit monochrome, PFNC lsb packed: two pixels in three bytes, filled
    /// from bit 0 of byte 0 (PFNC 2.4 §6.3.1).
    Mono12p = 0x010C_0047, "Mono12p";
    /// 10-bit monochrome in the GigE Vision 2.0 layout: two pixels in three
    /// bytes, the eight most significant bits of each in bytes 0 and 2, the
    /// two least significant bits in bits 1-0 and 5-4 of byte 1.
    Mono10Packed = 0x010C_0004, "Mono10Packed";
    /// 12-bit monochrome in the GigE Vision 2.0 layout: two pixels in three
    /// bytes, the eight most significant bits of each in bytes 0 and 2, the
    /// four least significant bits in the low and high nibble of byte 1. Not
    /// the bit order of [`PixelFormat::Mono12p`].
    ///
    /// EMVA's values table assigns the code to GigE Vision 2.0, whose text is
    /// not public; the layout is the one Teledyne FLIR documents for its
    /// cameras ("Understanding Pixel Formats"), as for
    /// [`PixelFormat::Mono10Packed`].
    Mono12Packed = 0x010C_0006, "Mono12Packed";
    Confidence8 = 0x0108_00C6, "Confidence8";
    Coord3DC16 = 0x0110_00B8, "Coord3D_C16";
    Coord3DC32f = 0x0120_00BF, "Coord3D_C32f";
    Coord3DAC16 = 0x0220_00BB, "Coord3D_AC16";
    Coord3DAC32f = 0x0240_00C2, "Coord3D_AC32f";
    Coord3DABC32f = 0x0260_00C0, "Coord3D_ABC32f";
    BayerGR8 = 0x0108_0008, "BayerGR8";
    BayerRG8 = 0x0108_0009, "BayerRG8";
    BayerGB8 = 0x0108_000A, "BayerGB8";
    BayerBG8 = 0x0108_000B, "BayerBG8";
    BayerGR10 = 0x0110_000C, "BayerGR10";
    BayerRG10 = 0x0110_000D, "BayerRG10";
    BayerGB10 = 0x0110_000E, "BayerGB10";
    BayerBG10 = 0x0110_000F, "BayerBG10";
    BayerGR12 = 0x0110_0010, "BayerGR12";
    BayerRG12 = 0x0110_0011, "BayerRG12";
    BayerGB12 = 0x0110_0012, "BayerGB12";
    BayerBG12 = 0x0110_0013, "BayerBG12";
    BayerGR16 = 0x0110_002E, "BayerGR16";
    BayerRG16 = 0x0110_002F, "BayerRG16";
    BayerGB16 = 0x0110_0030, "BayerGB16";
    BayerBG16 = 0x0110_0031, "BayerBG16";
    /// A Bayer mosaic packed as [`PixelFormat::Mono10p`] is.
    BayerGR10p = 0x010A_0056, "BayerGR10p";
    /// A Bayer mosaic packed as [`PixelFormat::Mono10p`] is.
    BayerRG10p = 0x010A_0058, "BayerRG10p";
    /// A Bayer mosaic packed as [`PixelFormat::Mono10p`] is.
    BayerGB10p = 0x010A_0054, "BayerGB10p";
    /// A Bayer mosaic packed as [`PixelFormat::Mono10p`] is.
    BayerBG10p = 0x010A_0052, "BayerBG10p";
    /// A Bayer mosaic packed as [`PixelFormat::Mono12p`] is.
    BayerGR12p = 0x010C_0057, "BayerGR12p";
    /// A Bayer mosaic packed as [`PixelFormat::Mono12p`] is.
    BayerRG12p = 0x010C_0059, "BayerRG12p";
    /// A Bayer mosaic packed as [`PixelFormat::Mono12p`] is.
    BayerGB12p = 0x010C_0055, "BayerGB12p";
    /// A Bayer mosaic packed as [`PixelFormat::Mono12p`] is.
    BayerBG12p = 0x010C_0053, "BayerBG12p";
    /// A Bayer mosaic packed as [`PixelFormat::Mono10Packed`] is.
    BayerGR10Packed = 0x010C_0026, "BayerGR10Packed";
    /// A Bayer mosaic packed as [`PixelFormat::Mono10Packed`] is.
    BayerRG10Packed = 0x010C_0027, "BayerRG10Packed";
    /// A Bayer mosaic packed as [`PixelFormat::Mono10Packed`] is.
    BayerGB10Packed = 0x010C_0028, "BayerGB10Packed";
    /// A Bayer mosaic packed as [`PixelFormat::Mono10Packed`] is.
    BayerBG10Packed = 0x010C_0029, "BayerBG10Packed";
    /// A Bayer mosaic packed as [`PixelFormat::Mono12Packed`] is.
    BayerGR12Packed = 0x010C_002A, "BayerGR12Packed";
    /// A Bayer mosaic packed as [`PixelFormat::Mono12Packed`] is.
    BayerRG12Packed = 0x010C_002B, "BayerRG12Packed";
    /// A Bayer mosaic packed as [`PixelFormat::Mono12Packed`] is.
    BayerGB12Packed = 0x010C_002C, "BayerGB12Packed";
    /// A Bayer mosaic packed as [`PixelFormat::Mono12Packed`] is.
    BayerBG12Packed = 0x010C_002D, "BayerBG12Packed";
    RGB8Packed = 0x0218_0014, "RGB8Packed" | "RGB8";
    BGR8Packed = 0x0218_0015, "BGR8Packed" | "BGR8";
    /// RGB with a fourth, manufacturer-defined alpha byte.
    RGBa8 = 0x0220_0016, "RGBa8" | "RGBA8Packed";
    /// BGR with a fourth, manufacturer-defined alpha byte.
    BGRa8 = 0x0220_0017, "BGRa8" | "BGRA8Packed";
    /// Y'CbCr 4:1:1, six bytes for four pixels in the order U Y Y V Y Y.
    YUV411_8_UYYVYY = 0x020C_001E, "YUV411_8_UYYVYY" | "YUV411Packed";
    /// Y'CbCr 4:2:2, four bytes for two pixels in the order U Y V Y.
    YUV422_8_UYVY = 0x0210_001F, "YUV422_8_UYVY" | "YUV422Packed";
    /// Y'CbCr 4:2:2, four bytes for two pixels in the order Y U Y V — the
    /// PFNC default for 4:2:2 (PFNC 2.4 §3.1.3).
    YUV422_8 = 0x0210_0032, "YUV422_8" | "YUV422_YUYV_Packed";
    /// Full-range Y'CbCr 4:2:2 in the order Y Cb Y Cr.
    YCbCr422_8 = 0x0210_003B, "YCbCr422_8";
    /// Full-range Y'CbCr 4:2:2 in the order Cb Y Cr Y.
    YCbCr422_8_CbYCrY = 0x0210_0043, "YCbCr422_8_CbYCrY";
    /// Y'CbCr 4:4:4 in the order U Y V.
    YUV8_UYV = 0x0218_0020, "YUV8_UYV" | "YUV444Packed";
    /// Full-range Y'CbCr 4:4:4 in the order Cb Y Cr.
    YCbCr8_CbYCr = 0x0218_003A, "YCbCr8_CbYCr";
    /// Full-range Y'CbCr 4:4:4 in the order Y Cb Cr.
    YCbCr8 = 0x0218_005B, "YCbCr8";
    /// Prophesee EVT 3.0 event stream, as a Lucid Triton2 EVS sends it
    /// ([#139](https://github.com/VitalyVorobyev/viva-genicam/pull/139)).
    ///
    /// **Vendor-defined, not PFNC**: bit 31 of the code is set, which is how
    /// PFNC marks a format outside its own table — hence the `VendorLucid`
    /// prefix, so a Lucid extension cannot read as a standard format name.
    /// The stream carries events,
    /// not pixels, so [`PixelFormat::bytes_per_pixel`] answers the size of one
    /// event *word* (16 bits) — the depth the code's own bits 23-16 declare,
    /// and the same answer [`PixelFormat::Unknown`] would derive for it. A
    /// block's length is not `width * height` times that.
    VendorLucidEvsEvt30 = 0x8110_0E30, "EVT3_0";
    /// Prophesee EVT 2.1 event stream, 64-bit event words. Vendor-defined; see
    /// [`PixelFormat::VendorLucidEvsEvt30`].
    VendorLucidEvsEvt21 = 0x8140_0E21, "EVT2_1";
}

impl PixelFormat {
    /// Number of bytes used to encode a single pixel.
    ///
    /// Derived from bits 23-16 of the PFNC code, which carry the pixel's bit
    /// depth, so a format this enumeration does not name is sized too. `None`
    /// means the pixel genuinely has no whole-byte size — a packed format —
    /// not that we failed to look it up; [`PixelFormat::bits_per_pixel`] and
    /// [`PixelFormat::image_bytes`] answer for those.
    pub const fn bytes_per_pixel(self) -> Option<usize> {
        PixelFormat::bytes_from_code(self.code())
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
    /// `BayerGR12Packed` and `BayerRG12Packed` all declare 12 bits, and they
    /// are common in the vendor corpus — `Mono12Packed` alone is offered by 28
    /// of its 38 documents. Rounding 12 bits up to 2 bytes overstates a frame
    /// by a third, which a length check downstream reads as a *short payload* —
    /// a confidently wrong size is worse than an absent one.
    const fn bytes_from_code(code: u32) -> Option<usize> {
        let bits = (code >> 16) & 0xFF;
        if bits == 0 || !bits.is_multiple_of(8) {
            return None;
        }
        Some((bits / 8) as usize)
    }

    /// Bits one pixel occupies, from bits 23-16 of the PFNC code: 12 for
    /// `Mono12p` and `Mono12Packed`, 16 for `Mono12` (two bytes, four of them
    /// padding) and for 4:2:2 Y'CbCr (an average over each pair of pixels).
    ///
    /// `None` only for a code that declares no size, such as `Unknown(0)`.
    pub const fn bits_per_pixel(self) -> Option<u32> {
        match (self.code() >> 16) & 0xFF {
            0 => None,
            bits => Some(bits),
        }
    }

    /// Bytes holding `width` pixels that start a packing group, the group
    /// completed when `width` ends inside one.
    ///
    /// PFNC 2.4 §6.7 completes a group the image or line ends inside with
    /// zero-valued components, so `Mono12p` holds 3 bytes for one pixel as
    /// for two. This is the length of one line under §6.7's *line padding*.
    /// `None` when [`PixelFormat::image_bytes`] is.
    pub const fn line_bytes(self, width: u32) -> Option<usize> {
        self.bytes_for(width as usize)
    }

    /// Bytes holding a `width` × `height` image whose pixels run on from one
    /// line to the next without padding (PFNC 2.4 §6.7, *image padding*), the
    /// last packing group completed with zero-valued pixels.
    ///
    /// Equal to `line_bytes(width) * height` whenever `width` is a multiple of
    /// the packing group: four pixels for 10p, two for 12p, `10Packed`,
    /// `12Packed` and 4:2:2, four for 4:1:1. `None` for an event stream, whose
    /// leader fields are not image geometry, for a code that declares no size,
    /// and for a size that overflows `usize`.
    pub const fn image_bytes(self, width: u32, height: u32) -> Option<usize> {
        let Some(pixels) = (width as usize).checked_mul(height as usize) else {
            return None;
        };
        self.bytes_for(pixels)
    }

    /// The smallest run of `(pixels, bytes)` that starts on a byte boundary.
    ///
    /// Subsampled Y'CbCr carries its chroma once per group (PFNC 2.4 §3.1.3,
    /// §3.1.4). Otherwise the group is as many pixels as fill whole bytes
    /// with no padding bit (§6.3.1 for `p`): four for 10 bits, two for 12;
    /// the GigE Vision `10Packed`/`12Packed` formats declare 12 bits and put
    /// two pixels in three bytes, which is the same group.
    const fn group(self) -> Option<(usize, usize)> {
        match self {
            PixelFormat::YUV422_8
            | PixelFormat::YUV422_8_UYVY
            | PixelFormat::YCbCr422_8
            | PixelFormat::YCbCr422_8_CbYCrY => return Some((2, 4)),
            PixelFormat::YUV411_8_UYYVYY => return Some((4, 6)),
            PixelFormat::VendorLucidEvsEvt30 | PixelFormat::VendorLucidEvsEvt21 => return None,
            _ => {}
        }
        let Some(bits) = self.bits_per_pixel() else {
            return None;
        };
        // gcd(bits, 8): the largest power of two dividing both.
        let common = 1u32 << bits.trailing_zeros();
        let common = if common > 8 { 8 } else { common };
        Some(((8 / common) as usize, (bits / common) as usize))
    }

    /// Bytes for `pixels` pixels starting on a group boundary, the last group
    /// completed.
    const fn bytes_for(self, pixels: usize) -> Option<usize> {
        let Some((group_pixels, group_bytes)) = self.group() else {
            return None;
        };
        pixels.div_ceil(group_pixels).checked_mul(group_bytes)
    }

    /// Whether the pixel format represents a Bayer mosaic.
    pub const fn is_bayer(self) -> bool {
        self.cfa_pattern().is_some()
    }

    /// Whether the pixel format is single-component monochrome (`Mono*`).
    pub const fn is_mono(self) -> bool {
        matches!(
            self,
            PixelFormat::Mono8
                | PixelFormat::Mono10
                | PixelFormat::Mono12
                | PixelFormat::Mono14
                | PixelFormat::Mono16
                | PixelFormat::Mono10p
                | PixelFormat::Mono12p
                | PixelFormat::Mono10Packed
                | PixelFormat::Mono12Packed
        )
    }

    /// Return the Color Filter Array pattern and canonical offsets.
    ///
    /// The tuple encodes `(pattern, x_offset, y_offset)` where the offsets
    /// describe how the sensor mosaic aligns to the canonical `"RGGB"`
    /// ordering: the pixel at `(x, y)` has the colour `"RGGB"` has at
    /// `(x + x_offset, y + y_offset)`. The first two letters of a Bayer name
    /// are its top row (PFNC 2.4 §3.1.7): `BayerGR` is G R over B G.
    pub const fn cfa_pattern(self) -> Option<(&'static str, u8, u8)> {
        use PixelFormat as P;
        match self {
            P::BayerRG8
            | P::BayerRG10
            | P::BayerRG12
            | P::BayerRG16
            | P::BayerRG10p
            | P::BayerRG12p
            | P::BayerRG10Packed
            | P::BayerRG12Packed => Some(("RGGB", 0, 0)),
            P::BayerGR8
            | P::BayerGR10
            | P::BayerGR12
            | P::BayerGR16
            | P::BayerGR10p
            | P::BayerGR12p
            | P::BayerGR10Packed
            | P::BayerGR12Packed => Some(("RGGB", 1, 0)),
            P::BayerGB8
            | P::BayerGB10
            | P::BayerGB12
            | P::BayerGB16
            | P::BayerGB10p
            | P::BayerGB12p
            | P::BayerGB10Packed
            | P::BayerGB12Packed => Some(("RGGB", 0, 1)),
            P::BayerBG8
            | P::BayerBG10
            | P::BayerBG12
            | P::BayerBG16
            | P::BayerBG10p
            | P::BayerBG12p
            | P::BayerBG10Packed
            | P::BayerBG12Packed => Some(("RGGB", 1, 1)),
            _ => None,
        }
    }
}

impl fmt::Display for PixelFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "Unknown(0x{:08X})", self.code()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PixelFormat;

    #[test]
    fn roundtrip_known_codes() {
        for &fmt in PixelFormat::ALL {
            assert_eq!(PixelFormat::from_code(fmt.code()), fmt);
            assert_eq!(PixelFormat::from_name(&fmt.to_string()), fmt);
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
        assert_eq!(PixelFormat::VendorLucidEvsEvt30.bytes_per_pixel(), Some(2));
        assert_eq!(PixelFormat::VendorLucidEvsEvt21.bytes_per_pixel(), Some(8));
        assert_eq!(PixelFormat::Unknown(0).bytes_per_pixel(), None);
    }

    #[test]
    fn event_format_names_roundtrip() {
        for (name, format) in [
            ("EVT3_0", PixelFormat::VendorLucidEvsEvt30),
            ("EVT2_1", PixelFormat::VendorLucidEvsEvt21),
        ] {
            assert_eq!(PixelFormat::from_name(name), format);
            assert_eq!(format.to_string(), name);
        }
    }

    /// Every named format must agree with the size encoded in its own code,
    /// which is what makes the `Unknown` derivation trustworthy.
    #[test]
    fn named_formats_agree_with_their_own_pfnc_code() {
        for &fmt in PixelFormat::ALL {
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
        // RGB8_Planar: 24 bits. RGB16: 48 bits. Coord3D_ABC32: 96 bits.
        assert_eq!(
            PixelFormat::from_code(0x0218_0021).bytes_per_pixel(),
            Some(3)
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
    /// 28 of its 38 documents. Rounding 12 bits up to 2 bytes would
    /// overstate a frame by a third and make a length check downstream reject
    /// it as short.
    #[test]
    fn packed_formats_report_no_whole_byte_size() {
        for (name, code) in [
            ("Mono10Packed", 0x010C_0004_u32),
            ("Mono12Packed", 0x010C_0006),
            ("YUV411Packed", 0x020C_001E),
            ("BayerGB12Packed", 0x010C_002C),
            ("BayerBG12Packed", 0x010C_002D),
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

    /// Every name and code below is copied from EMVA's "GenICam Pixel Format
    /// Names and Values" (7 October 2025), the table PFNC 2.4 Appendix C
    /// points to. A name must map to its code and the code back to the name.
    #[test]
    fn names_and_codes_match_the_emva_values_table() {
        for (name, code) in [
            ("Mono10p", 0x010A_0046_u32),
            ("Mono12p", 0x010C_0047),
            ("Mono10Packed", 0x010C_0004),
            ("Mono12Packed", 0x010C_0006),
            ("BayerGR10", 0x0110_000C),
            ("BayerRG10", 0x0110_000D),
            ("BayerGB10", 0x0110_000E),
            ("BayerBG10", 0x0110_000F),
            ("BayerGR12", 0x0110_0010),
            ("BayerRG12", 0x0110_0011),
            ("BayerGB12", 0x0110_0012),
            ("BayerBG12", 0x0110_0013),
            ("BayerGR16", 0x0110_002E),
            ("BayerRG16", 0x0110_002F),
            ("BayerGB16", 0x0110_0030),
            ("BayerBG16", 0x0110_0031),
            ("BayerGR10Packed", 0x010C_0026),
            ("BayerRG10Packed", 0x010C_0027),
            ("BayerGB10Packed", 0x010C_0028),
            ("BayerBG10Packed", 0x010C_0029),
            ("BayerGR12Packed", 0x010C_002A),
            ("BayerRG12Packed", 0x010C_002B),
            ("BayerGB12Packed", 0x010C_002C),
            ("BayerBG12Packed", 0x010C_002D),
            ("BayerBG10p", 0x010A_0052),
            ("BayerBG12p", 0x010C_0053),
            ("BayerGB10p", 0x010A_0054),
            ("BayerGB12p", 0x010C_0055),
            ("BayerGR10p", 0x010A_0056),
            ("BayerGR12p", 0x010C_0057),
            ("BayerRG10p", 0x010A_0058),
            ("BayerRG12p", 0x010C_0059),
            ("RGBa8", 0x0220_0016),
            ("BGRa8", 0x0220_0017),
            ("YUV411_8_UYYVYY", 0x020C_001E),
            ("YUV422_8_UYVY", 0x0210_001F),
            ("YUV422_8", 0x0210_0032),
            ("YCbCr422_8", 0x0210_003B),
            ("YCbCr422_8_CbYCrY", 0x0210_0043),
            ("YUV8_UYV", 0x0218_0020),
            ("YCbCr8_CbYCr", 0x0218_003A),
            ("YCbCr8", 0x0218_005B),
            ("Coord3D_C16", 0x0110_00B8),
        ] {
            assert_eq!(PixelFormat::from_name(name).code(), code, "{name}");
            assert_eq!(
                PixelFormat::from_code(code).to_string(),
                name,
                "{code:#010X}"
            );
        }
    }

    /// The Bayer 16-bit variants carried the codes of BayerGB10, BayerBG10,
    /// BayerGR12 and BayerRG12, so a BayerGB10 camera was read as BayerGR16:
    /// the wrong mosaic, drawn six bits too dark.
    #[test]
    fn bayer16_codes_are_not_the_bayer10_and_12_codes() {
        assert_eq!(PixelFormat::BayerGR16.code(), 0x0110_002E);
        assert_eq!(PixelFormat::BayerRG16.code(), 0x0110_002F);
        assert_eq!(PixelFormat::BayerGB16.code(), 0x0110_0030);
        assert_eq!(PixelFormat::BayerBG16.code(), 0x0110_0031);
        assert_eq!(PixelFormat::from_code(0x0110_000E).to_string(), "BayerGB10");
        assert_eq!(PixelFormat::from_code(0x0110_0011).to_string(), "BayerRG12");
    }

    /// The names cameras actually use for the same codes: the GigE Vision 1.x
    /// spellings SFNC kept, found in the vendor XML corpus (`YUV422Packed` in
    /// 23 of 38 documents, `RGB8Packed` in 23).
    #[test]
    fn legacy_names_resolve_to_the_pfnc_codes() {
        for (name, code) in [
            ("YUV422Packed", 0x0210_001F_u32),
            ("YUV422_YUYV_Packed", 0x0210_0032),
            ("YUV411Packed", 0x020C_001E),
            ("YUV444Packed", 0x0218_0020),
            ("RGBA8Packed", 0x0220_0016),
            ("BGRA8Packed", 0x0220_0017),
            ("RGB8Packed", 0x0218_0014),
            ("BGR8Packed", 0x0218_0015),
        ] {
            assert_eq!(PixelFormat::from_name(name).code(), code, "{name}");
        }
    }

    /// PFNC 2.4 §3.1.7, Figures 3-7 to 3-14: the two letters after `Bayer`
    /// are the top row of the 2×2 tile, whatever the depth or packing.
    #[test]
    fn cfa_offsets_align_to_rggb() {
        use PixelFormat as P;
        for (formats, offsets) in [
            (
                [P::BayerRG8, P::BayerRG10, P::BayerRG12, P::BayerRG16],
                ("RGGB", 0, 0),
            ),
            (
                [P::BayerGR8, P::BayerGR10, P::BayerGR12, P::BayerGR16],
                ("RGGB", 1, 0),
            ),
            (
                [P::BayerGB8, P::BayerGB10, P::BayerGB12, P::BayerGB16],
                ("RGGB", 0, 1),
            ),
            (
                [P::BayerBG8, P::BayerBG10, P::BayerBG12, P::BayerBG16],
                ("RGGB", 1, 1),
            ),
            (
                [
                    P::BayerRG10p,
                    P::BayerRG12p,
                    P::BayerRG10Packed,
                    P::BayerRG12Packed,
                ],
                ("RGGB", 0, 0),
            ),
            (
                [
                    P::BayerGR10p,
                    P::BayerGR12p,
                    P::BayerGR10Packed,
                    P::BayerGR12Packed,
                ],
                ("RGGB", 1, 0),
            ),
            (
                [
                    P::BayerGB10p,
                    P::BayerGB12p,
                    P::BayerGB10Packed,
                    P::BayerGB12Packed,
                ],
                ("RGGB", 0, 1),
            ),
            (
                [
                    P::BayerBG10p,
                    P::BayerBG12p,
                    P::BayerBG10Packed,
                    P::BayerBG12Packed,
                ],
                ("RGGB", 1, 1),
            ),
        ] {
            for format in formats {
                assert_eq!(format.cfa_pattern(), Some(offsets), "{format}");
                assert!(format.is_bayer() && !format.is_mono(), "{format}");
            }
        }
        assert_eq!(P::Mono8.cfa_pattern(), None);
        assert_eq!(P::Mono12Packed.cfa_pattern(), None);
        // Every named Bayer format has a pattern.
        for &format in P::ALL {
            assert_eq!(
                format.is_bayer(),
                format.to_string().starts_with("Bayer"),
                "{format}"
            );
        }
    }

    /// Bits 23-16 of the code (PFNC values table): the container a packed
    /// format fills, 16 for a 12-bit pixel padded to two bytes.
    #[test]
    fn bits_per_pixel_is_the_size_field_of_the_code() {
        use PixelFormat as P;
        for (format, bits) in [
            (P::Mono8, 8),
            (P::Mono10p, 10),
            (P::Mono12p, 12),
            (P::Mono10Packed, 12),
            (P::Mono12Packed, 12),
            (P::Mono12, 16),
            (P::BayerRG12Packed, 12),
            (P::YUV411_8_UYYVYY, 12),
            (P::YUV422_8, 16),
            (P::RGB8Packed, 24),
            (P::RGBa8, 32),
        ] {
            assert_eq!(format.bits_per_pixel(), Some(bits), "{format}");
        }
        assert_eq!(P::Unknown(0).bits_per_pixel(), None);
        assert_eq!(P::from_code(0x0110_FFFF).bits_per_pixel(), Some(16));
    }

    /// A line of `width` pixels, its last packing group completed with zero
    /// components (PFNC 2.4 §6.7). Groups: §6.3.1 packs "as many components
    /// such that no padding bit is required" — 4 for 10p, 2 for 12p; GigE
    /// Vision's 10/12Packed put 2 pixels in 3 bytes; 4:2:2 carries 2 pixels
    /// per Cb/Cr pair (§3.1.3), 4:1:1 four (§3.1.4).
    #[test]
    fn line_bytes_complete_the_last_packing_group() {
        use PixelFormat as P;
        for (format, width, bytes) in [
            (P::Mono8, 7, 7),
            (P::Mono12, 3, 6),
            (P::RGB8Packed, 2, 6),
            (P::Mono10p, 4, 5),
            (P::Mono10p, 8, 10),
            (P::Mono10p, 1, 5),
            (P::Mono10p, 5, 10),
            (P::Mono12p, 2, 3),
            (P::Mono12p, 3, 6),
            (P::Mono12Packed, 640, 960),
            (P::Mono12Packed, 1, 3),
            (P::Mono10Packed, 2, 3),
            (P::BayerGR10p, 4, 5),
            (P::YUV422_8_UYVY, 2, 4),
            (P::YUV422_8, 3, 8),
            (P::YUV411_8_UYYVYY, 4, 6),
            (P::YUV411_8_UYYVYY, 5, 12),
            (P::YUV8_UYV, 3, 9),
        ] {
            assert_eq!(format.line_bytes(width), Some(bytes), "{format} × {width}");
        }
    }

    /// The image as one run of pixels (PFNC 2.4 §6.7, image padding): only the
    /// last group is completed, not every line's.
    #[test]
    fn image_bytes_complete_only_the_last_group() {
        use PixelFormat as P;
        for (format, width, height, bytes) in [
            (P::Mono8, 640, 480, 307_200),
            (P::Mono16, 640, 480, 614_400),
            (P::Mono12Packed, 640, 480, 460_800),
            (P::Mono12p, 640, 480, 460_800),
            (P::Mono10p, 640, 480, 384_000),
            (P::Mono10p, 2, 2, 5),
            (P::Mono10p, 3, 3, 15),
            (P::Mono12p, 3, 1, 6),
            (P::YUV422_8, 640, 480, 614_400),
            (P::YUV422_8, 3, 1, 8),
            (P::Coord3DABC32f, 1280, 1, 15_360),
        ] {
            assert_eq!(
                format.image_bytes(width, height),
                Some(bytes),
                "{format} {width}×{height}"
            );
        }
        // Event blocks have no image geometry.
        assert_eq!(P::VendorLucidEvsEvt30.image_bytes(1, 64_000), None);
        assert_eq!(P::VendorLucidEvsEvt21.image_bytes(1, 64_000), None);
        assert_eq!(P::Unknown(0).image_bytes(4, 4), None);
        // An unknown code is sized from its own size field.
        assert_eq!(P::from_code(0x010C_FFFF).image_bytes(2, 1), Some(3));
    }

    #[test]
    fn image_bytes_refuse_to_overflow() {
        assert_eq!(
            PixelFormat::Coord3DABC32f.image_bytes(u32::MAX, u32::MAX),
            None
        );
    }
}
