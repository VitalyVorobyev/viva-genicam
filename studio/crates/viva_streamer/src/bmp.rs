//! BMP encoder for 8-bit grayscale (Mono8) and 24-bit RGB (colour formats).
//!
//! Two encoder modes:
//!
//! - **8bpp grayscale** (`BmpEncoder::new`): BITMAPFILEHEADER + BITMAPINFOHEADER +
//!   256-entry grayscale palette + 1-byte-per-pixel data with 4-byte row alignment.
//!   Used for Mono8 and downscaled Mono10/12/16.
//!
//! - **24bpp RGB** (`BmpEncoder::new_rgb24`): BITMAPFILEHEADER + BITMAPINFOHEADER +
//!   no palette + 3-bytes-per-pixel BGR data with 4-byte row alignment.
//!   Used for RGB8, BGR8, RGBa8, and debayered Bayer images.
//!
//! Both modes use a *top-down* DIB (negative height in the header) so rows are
//! written in top-first order without flipping.
//!
//! Utility functions for converting other formats to the required pixel buffer:
//!
//! - [`mono_u16le_to_gray8`]: downscales Mono10/12/16 packed-LE pixels to 8-bit.
//! - [`debayer_nn`]: bilinear-interpolated Bayer demosaicing to RGB8.

use bytes::Bytes;
use thiserror::Error;

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum BmpError {
    #[error("pixel buffer size mismatch: expected {expected} bytes, got {actual}")]
    SizeMismatch { expected: usize, actual: usize },
    #[error("pixel buffer size overflow for width*height")]
    SizeOverflow,
}

// ── BmpEncoder ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BmpEncoder {
    pub width: u32,
    pub height: u32,
    /// Pre-built BMP file + DIB header (and palette for 8bpp).
    pub header_prefix: Vec<u8>,
    /// Row stride in bytes (width in bytes rounded up to 4-byte boundary).
    pub row_stride: usize,
    /// Total pixel data size in bytes.
    pub image_size: usize,
}

impl BmpEncoder {
    /// Create an 8-bit grayscale BMP encoder.
    ///
    /// Produces a palette-indexed BMP with a 256-entry identity grayscale palette.
    pub fn new(width: u32, height: u32) -> Self {
        // 8-bit BMP rows are padded to 4-byte alignment.
        let row_stride = (width as usize).div_ceil(4) * 4;
        let image_size = row_stride * height as usize;

        let palette_size = 256 * 4;
        let header_size = 14 + 40 + palette_size;
        let file_size = header_size + image_size;

        let mut header_prefix = Vec::with_capacity(header_size);

        // BITMAPFILEHEADER (14 bytes)
        header_prefix.extend_from_slice(b"BM");
        header_prefix.extend_from_slice(&(file_size as u32).to_le_bytes());
        header_prefix.extend_from_slice(&0u16.to_le_bytes()); // reserved1
        header_prefix.extend_from_slice(&0u16.to_le_bytes()); // reserved2
        header_prefix.extend_from_slice(&(header_size as u32).to_le_bytes()); // pixel data offset

        // BITMAPINFOHEADER (40 bytes)
        header_prefix.extend_from_slice(&(40u32).to_le_bytes()); // header size
        header_prefix.extend_from_slice(&(width as i32).to_le_bytes());
        // Negative height => top-down DIB (no row flip needed).
        header_prefix.extend_from_slice(&(-(height as i32)).to_le_bytes());
        header_prefix.extend_from_slice(&(1u16).to_le_bytes()); // planes
        header_prefix.extend_from_slice(&(8u16).to_le_bytes()); // bits per pixel
        header_prefix.extend_from_slice(&(0u32).to_le_bytes()); // compression (BI_RGB)
        header_prefix.extend_from_slice(&(image_size as u32).to_le_bytes());
        header_prefix.extend_from_slice(&0u32.to_le_bytes()); // x pixels per meter
        header_prefix.extend_from_slice(&0u32.to_le_bytes()); // y pixels per meter
        header_prefix.extend_from_slice(&(256u32).to_le_bytes()); // colors used
        header_prefix.extend_from_slice(&0u32.to_le_bytes()); // important colors

        // 256-entry grayscale palette (B, G, R, A=0)
        for value in 0u8..=255 {
            header_prefix.push(value);
            header_prefix.push(value);
            header_prefix.push(value);
            header_prefix.push(0);
        }

        Self {
            width,
            height,
            header_prefix,
            row_stride,
            image_size,
        }
    }

    /// Create a 24-bit RGB BMP encoder (no palette).
    ///
    /// Produces a true-colour BMP. Input pixels are expected as packed RGB
    /// (3 bytes per pixel); the encoder writes them as BGR as required by BMP.
    pub fn new_rgb24(width: u32, height: u32) -> Self {
        // 24-bit rows: 3 bytes per pixel, padded to 4-byte alignment.
        let row_stride = ((width as usize * 3) + 3) & !3;
        let image_size = row_stride * height as usize;

        // 24bpp BMP has no colour palette.
        let header_size = 14 + 40;
        let file_size = header_size + image_size;

        let mut header_prefix = Vec::with_capacity(header_size);

        // BITMAPFILEHEADER (14 bytes)
        header_prefix.extend_from_slice(b"BM");
        header_prefix.extend_from_slice(&(file_size as u32).to_le_bytes());
        header_prefix.extend_from_slice(&0u16.to_le_bytes()); // reserved1
        header_prefix.extend_from_slice(&0u16.to_le_bytes()); // reserved2
        header_prefix.extend_from_slice(&(header_size as u32).to_le_bytes()); // pixel data offset

        // BITMAPINFOHEADER (40 bytes)
        header_prefix.extend_from_slice(&(40u32).to_le_bytes()); // header size
        header_prefix.extend_from_slice(&(width as i32).to_le_bytes());
        // Negative height => top-down DIB.
        header_prefix.extend_from_slice(&(-(height as i32)).to_le_bytes());
        header_prefix.extend_from_slice(&(1u16).to_le_bytes()); // planes
        header_prefix.extend_from_slice(&(24u16).to_le_bytes()); // bits per pixel
        header_prefix.extend_from_slice(&(0u32).to_le_bytes()); // compression (BI_RGB)
        header_prefix.extend_from_slice(&(image_size as u32).to_le_bytes());
        header_prefix.extend_from_slice(&0u32.to_le_bytes()); // x pixels per meter
        header_prefix.extend_from_slice(&0u32.to_le_bytes()); // y pixels per meter
        header_prefix.extend_from_slice(&(0u32).to_le_bytes()); // colors used (0 = all)
        header_prefix.extend_from_slice(&0u32.to_le_bytes()); // important colors

        Self {
            width,
            height,
            header_prefix,
            row_stride,
            image_size,
        }
    }

    /// Encode a grayscale 8-bit pixel buffer as a complete BMP file.
    ///
    /// `pixels` must be exactly `width × height` bytes (row-major, top row first).
    pub fn encode_gray8(&self, pixels: &[u8]) -> Result<Bytes, BmpError> {
        let expected = (self.width as usize)
            .checked_mul(self.height as usize)
            .ok_or(BmpError::SizeOverflow)?;

        if pixels.len() != expected {
            return Err(BmpError::SizeMismatch {
                expected,
                actual: pixels.len(),
            });
        }

        let mut buffer = Vec::with_capacity(self.header_prefix.len() + self.image_size);
        buffer.extend_from_slice(&self.header_prefix);

        let row_width = self.width as usize;
        let padding = self.row_stride.saturating_sub(row_width);
        let pad_bytes = [0u8; 4];

        for row in 0..self.height as usize {
            let start = row * row_width;
            let end = start + row_width;
            buffer.extend_from_slice(&pixels[start..end]);
            if padding > 0 {
                buffer.extend_from_slice(&pad_bytes[..padding]);
            }
        }

        Ok(Bytes::from(buffer))
    }

    /// Encode an RGB8 pixel buffer as a 24bpp BMP file.
    ///
    /// `pixels` must be exactly `width × height × 3` bytes (row-major, R G B order).
    /// The encoder swaps to BGR as required by the BMP format.
    pub fn encode_rgb24(&self, pixels: &[u8]) -> Result<Bytes, BmpError> {
        let expected = (self.width as usize)
            .checked_mul(self.height as usize)
            .ok_or(BmpError::SizeOverflow)?
            .checked_mul(3)
            .ok_or(BmpError::SizeOverflow)?;

        if pixels.len() != expected {
            return Err(BmpError::SizeMismatch {
                expected,
                actual: pixels.len(),
            });
        }

        let mut buffer = Vec::with_capacity(self.header_prefix.len() + self.image_size);
        buffer.extend_from_slice(&self.header_prefix);

        let row_width_bytes = self.width as usize * 3;
        let padding = self.row_stride.saturating_sub(row_width_bytes);
        let pad_bytes = [0u8; 4];

        for row in 0..self.height as usize {
            let start = row * row_width_bytes;
            let end = start + row_width_bytes;
            let row_pixels = &pixels[start..end];

            // BMP stores BGR, input is RGB — swap R and B for each triplet.
            for [r, g, b] in row_pixels.as_chunks::<3>().0 {
                buffer.push(*b); // B
                buffer.push(*g); // G
                buffer.push(*r); // R
            }

            if padding > 0 {
                buffer.extend_from_slice(&pad_bytes[..padding]);
            }
        }

        Ok(Bytes::from(buffer))
    }
}

// ── Format conversion utilities ───────────────────────────────────────────────

/// Convert a 2-byte-per-pixel little-endian buffer (Mono10/12/16) to 8-bit gray.
///
/// The `src_bits` parameter is the significant bit depth (10, 12, or 16).
/// Each 16-bit LE sample is right-shifted by `src_bits - 8` to produce a u8.
///
/// # Panics
///
/// Does not panic; odd-length input is silently truncated (last byte ignored).
pub fn mono_u16le_to_gray8(pixels: &[u8], src_bits: u8) -> Vec<u8> {
    let shift = src_bits.saturating_sub(8);
    pixels
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| {
            let value = u16::from_le_bytes(*chunk);
            (value >> shift) as u8
        })
        .collect()
}

/// Bayer colour filter array pattern.
///
/// Names follow the GenICam SFNC convention: the two characters name the
/// top-left 2×1 pixel pair in the first row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BayerPattern {
    /// RG / GB — BayerRG in SFNC
    Rggb,
    /// GR / BG — BayerGR in SFNC
    Grbg,
    /// BG / GR — BayerBG in SFNC
    Bggr,
    /// GB / RG — BayerGB in SFNC
    Gbrg,
}

/// Bilinear-interpolated Bayer demosaicing.
///
/// Returns a packed RGB8 buffer (3 bytes per pixel, R G B, row-major).
/// Border pixels clamp to the image edge. Input `pixels` must be exactly
/// `width × height` bytes (1 byte per pixel, raw Bayer mosaic, top row first).
pub fn debayer_nn(pixels: &[u8], width: u32, height: u32, pattern: BayerPattern) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut out = vec![0u8; w * h * 3];

    for y in 0..h {
        for x in 0..w {
            let xi = x as isize;
            let yi = y as isize;

            // Clamped pixel accessor.
            let p = |dx: isize, dy: isize| -> u16 {
                let cx = (xi + dx).clamp(0, w as isize - 1) as usize;
                let cy = (yi + dy).clamp(0, h as isize - 1) as usize;
                pixels[cy * w + cx] as u16
            };

            let (r, g, b): (u8, u8, u8) = match (pattern, x & 1, y & 1) {
                // ── RGGB ──────────────────────────────────────────────────
                (BayerPattern::Rggb, 0, 0) => (
                    p(0, 0) as u8,
                    ((p(-1, 0) + p(1, 0) + p(0, -1) + p(0, 1)) / 4) as u8,
                    ((p(-1, -1) + p(1, -1) + p(-1, 1) + p(1, 1)) / 4) as u8,
                ),
                (BayerPattern::Rggb, 1, 0) => (
                    ((p(-1, 0) + p(1, 0)) / 2) as u8,
                    p(0, 0) as u8,
                    ((p(0, -1) + p(0, 1)) / 2) as u8,
                ),
                (BayerPattern::Rggb, 0, 1) => (
                    ((p(0, -1) + p(0, 1)) / 2) as u8,
                    p(0, 0) as u8,
                    ((p(-1, 0) + p(1, 0)) / 2) as u8,
                ),
                (BayerPattern::Rggb, _, _) => (
                    // (1,1) — B pixel
                    ((p(-1, -1) + p(1, -1) + p(-1, 1) + p(1, 1)) / 4) as u8,
                    ((p(-1, 0) + p(1, 0) + p(0, -1) + p(0, 1)) / 4) as u8,
                    p(0, 0) as u8,
                ),
                // ── BGGR ──────────────────────────────────────────────────
                (BayerPattern::Bggr, 0, 0) => (
                    // (0,0) — B pixel; R is diagonal
                    ((p(-1, -1) + p(1, -1) + p(-1, 1) + p(1, 1)) / 4) as u8,
                    ((p(-1, 0) + p(1, 0) + p(0, -1) + p(0, 1)) / 4) as u8,
                    p(0, 0) as u8,
                ),
                (BayerPattern::Bggr, 1, 0) => (
                    // (1,0) — G on B-row
                    ((p(0, -1) + p(0, 1)) / 2) as u8,
                    p(0, 0) as u8,
                    ((p(-1, 0) + p(1, 0)) / 2) as u8,
                ),
                (BayerPattern::Bggr, 0, 1) => (
                    // (0,1) — G on R-row
                    ((p(-1, 0) + p(1, 0)) / 2) as u8,
                    p(0, 0) as u8,
                    ((p(0, -1) + p(0, 1)) / 2) as u8,
                ),
                (BayerPattern::Bggr, _, _) => (
                    // (1,1) — R pixel
                    p(0, 0) as u8,
                    ((p(-1, 0) + p(1, 0) + p(0, -1) + p(0, 1)) / 4) as u8,
                    ((p(-1, -1) + p(1, -1) + p(-1, 1) + p(1, 1)) / 4) as u8,
                ),
                // ── GRBG ──────────────────────────────────────────────────
                (BayerPattern::Grbg, 0, 0) => (
                    // (0,0) — G on R-row
                    ((p(-1, 0) + p(1, 0)) / 2) as u8,
                    p(0, 0) as u8,
                    ((p(0, -1) + p(0, 1)) / 2) as u8,
                ),
                (BayerPattern::Grbg, 1, 0) => (
                    // (1,0) — R pixel
                    p(0, 0) as u8,
                    ((p(-1, 0) + p(1, 0) + p(0, -1) + p(0, 1)) / 4) as u8,
                    ((p(-1, -1) + p(1, -1) + p(-1, 1) + p(1, 1)) / 4) as u8,
                ),
                (BayerPattern::Grbg, 0, 1) => (
                    // (0,1) — B pixel
                    ((p(-1, -1) + p(1, -1) + p(-1, 1) + p(1, 1)) / 4) as u8,
                    ((p(-1, 0) + p(1, 0) + p(0, -1) + p(0, 1)) / 4) as u8,
                    p(0, 0) as u8,
                ),
                (BayerPattern::Grbg, _, _) => (
                    // (1,1) — G on B-row
                    ((p(0, -1) + p(0, 1)) / 2) as u8,
                    p(0, 0) as u8,
                    ((p(-1, 0) + p(1, 0)) / 2) as u8,
                ),
                // ── GBRG ──────────────────────────────────────────────────
                (BayerPattern::Gbrg, 0, 0) => (
                    // (0,0) — G on B-row
                    ((p(0, -1) + p(0, 1)) / 2) as u8,
                    p(0, 0) as u8,
                    ((p(-1, 0) + p(1, 0)) / 2) as u8,
                ),
                (BayerPattern::Gbrg, 1, 0) => (
                    // (1,0) — B pixel
                    ((p(-1, -1) + p(1, -1) + p(-1, 1) + p(1, 1)) / 4) as u8,
                    ((p(-1, 0) + p(1, 0) + p(0, -1) + p(0, 1)) / 4) as u8,
                    p(0, 0) as u8,
                ),
                (BayerPattern::Gbrg, 0, 1) => (
                    // (0,1) — R pixel
                    p(0, 0) as u8,
                    ((p(-1, 0) + p(1, 0) + p(0, -1) + p(0, 1)) / 4) as u8,
                    ((p(-1, -1) + p(1, -1) + p(-1, 1) + p(1, 1)) / 4) as u8,
                ),
                (BayerPattern::Gbrg, _, _) => (
                    // (1,1) — G on R-row
                    ((p(-1, 0) + p(1, 0)) / 2) as u8,
                    p(0, 0) as u8,
                    ((p(0, -1) + p(0, 1)) / 2) as u8,
                ),
            };

            let idx = (y * w + x) * 3;
            out[idx] = r;
            out[idx + 1] = g;
            out[idx + 2] = b;
        }
    }

    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Gray8 encoder (existing) ──────────────────────────────────────────────

    #[test]
    fn encode_2x2_has_valid_headers_and_palette() {
        let encoder = BmpEncoder::new(2, 2);
        let pixels = [0u8, 127, 255, 64];
        let bytes = encoder.encode_gray8(&pixels).expect("encode succeeds");

        // BMP signature
        assert_eq!(&bytes[0..2], b"BM");

        // File size matches buffer length.
        let file_size = u32::from_le_bytes(bytes[2..6].try_into().expect("file size"));
        assert_eq!(file_size as usize, bytes.len());

        // Pixel data offset should include 14 + 40 + 256*4 bytes.
        let offset = u32::from_le_bytes(bytes[10..14].try_into().expect("offset"));
        assert_eq!(offset, 14 + 40 + 256 * 4);

        // Palette sanity check: first and last entries are grayscale.
        let palette_start = 14 + 40;
        let palette_end = palette_start + 256 * 4;
        assert!(bytes.len() >= palette_end);
        assert_eq!(&bytes[palette_start..palette_start + 4], &[0, 0, 0, 0]);
        assert_eq!(&bytes[palette_end - 4..palette_end], &[255, 255, 255, 0]);

        // Pixel data is top-down and padded to 4-byte rows (row_stride = 4).
        let data = &bytes[offset as usize..];
        assert_eq!(&data[0..2], &pixels[0..2]); // first row
        assert_eq!(&data[4..6], &pixels[2..4]); // second row (after padding)
    }

    // ── RGB24 encoder ─────────────────────────────────────────────────────────

    #[test]
    fn test_new_rgb24_header_bits_per_pixel() {
        let encoder = BmpEncoder::new_rgb24(4, 4);
        // Bits per pixel at offset 28-29 (u16 LE) inside BITMAPINFOHEADER
        let bpp = u16::from_le_bytes([encoder.header_prefix[28], encoder.header_prefix[29]]);
        assert_eq!(bpp, 24, "24bpp header must have bits-per-pixel = 24");
    }

    #[test]
    fn test_encode_rgb24_1x1_pixel() {
        // Single pixel R=10, G=20, B=30 → BMP pixel data must be BGR = [30, 20, 10]
        let encoder = BmpEncoder::new_rgb24(1, 1);
        let pixels = [10u8, 20, 30]; // R, G, B
        let bytes = encoder.encode_rgb24(&pixels).expect("encode succeeds");

        // BMP signature
        assert_eq!(&bytes[0..2], b"BM");

        // Pixel data offset at bytes 10-13
        let offset = u32::from_le_bytes(bytes[10..14].try_into().unwrap()) as usize;
        // 1×1 24bpp: row_stride = 4 (padded from 3 to 4)
        assert!(bytes.len() >= offset + 4);
        assert_eq!(bytes[offset], 30, "B channel");
        assert_eq!(bytes[offset + 1], 20, "G channel");
        assert_eq!(bytes[offset + 2], 10, "R channel");
    }

    #[test]
    fn test_encode_rgb24_size_mismatch() {
        let encoder = BmpEncoder::new_rgb24(2, 2);
        // Correct size is 2*2*3 = 12; pass 9 bytes → SizeMismatch
        let result = encoder.encode_rgb24(&[0u8; 9]);
        assert!(matches!(
            result,
            Err(BmpError::SizeMismatch {
                expected: 12,
                actual: 9
            })
        ));
    }

    #[test]
    fn test_encode_rgb24_row_padding() {
        // 1-pixel-wide image: row is 3 bytes but padded to 4.
        let encoder = BmpEncoder::new_rgb24(1, 2);
        let pixels = [255u8, 0, 0, 0, 255, 0]; // row0: red, row1: green
        let bytes = encoder.encode_rgb24(&pixels).expect("encode succeeds");

        let offset = u32::from_le_bytes(bytes[10..14].try_into().unwrap()) as usize;
        // Row stride = 4, two rows → image_size = 8
        assert_eq!(encoder.row_stride, 4);
        assert_eq!(bytes.len(), offset + 8);

        // row0: B=0,G=0,R=255 then padding byte
        assert_eq!(bytes[offset], 0); // B
        assert_eq!(bytes[offset + 1], 0); // G
        assert_eq!(bytes[offset + 2], 255); // R
        // row1 starts at offset+4
        assert_eq!(bytes[offset + 4], 0); // B
        assert_eq!(bytes[offset + 5], 255); // G
        assert_eq!(bytes[offset + 6], 0); // R
    }

    // ── mono_u16le_to_gray8 ───────────────────────────────────────────────────

    #[test]
    fn test_mono_u16le_to_gray8_mono16() {
        // 0xFFFF >> 8 = 0xFF
        let pixels = [0xFFu8, 0xFF];
        assert_eq!(mono_u16le_to_gray8(&pixels, 16), vec![0xFF]);
    }

    #[test]
    fn test_mono_u16le_to_gray8_mono12() {
        // 0x0FFF (12-bit max) >> 4 = 0xFF
        let pixels = [0xFFu8, 0x0F];
        assert_eq!(mono_u16le_to_gray8(&pixels, 12), vec![0xFF]);
    }

    #[test]
    fn test_mono_u16le_to_gray8_mono10() {
        // 0x03FF (10-bit max) >> 2 = 0xFF
        let pixels = [0xFFu8, 0x03];
        assert_eq!(mono_u16le_to_gray8(&pixels, 10), vec![0xFF]);
    }

    #[test]
    fn test_mono_u16le_to_gray8_mid_value() {
        // 0x8000 >> 8 = 0x80
        let pixels = [0x00u8, 0x80];
        assert_eq!(mono_u16le_to_gray8(&pixels, 16), vec![0x80]);
    }

    #[test]
    fn test_mono_u16le_to_gray8_two_pixels() {
        let pixels = [0xFF, 0xFF, 0x00, 0x80];
        assert_eq!(mono_u16le_to_gray8(&pixels, 16), vec![0xFF, 0x80]);
    }

    // ── debayer_nn ────────────────────────────────────────────────────────────

    /// Flat RGGB 2×2 mosaic: top-left R, top-right G, bottom-left G, bottom-right B.
    /// With a constant-value image (all 200) the output should be all-200 after
    /// bilinear interpolation.
    #[test]
    fn test_debayer_nn_rggb_uniform() {
        let pixels = [200u8; 4];
        let rgb = debayer_nn(&pixels, 2, 2, BayerPattern::Rggb);
        // All channels should be 200 (or close, due to bilinear clamping at border)
        assert_eq!(rgb.len(), 12);
        // All values should be in [195, 205] given that all source pixels are 200
        for &v in &rgb {
            assert!((195..=205).contains(&v), "Expected ~200 but got {v}");
        }
    }

    #[test]
    fn test_debayer_nn_rggb_2x2_r_at_origin() {
        // All-zero except R at (0,0) = 200; check that R channel at (0,0) is 200.
        let pixels = [200u8, 0, 0, 0]; // RGGB: (0,0)=R, (1,0)=G, (0,1)=G, (1,1)=B
        let rgb = debayer_nn(&pixels, 2, 2, BayerPattern::Rggb);
        // Pixel (0,0): R=200
        assert_eq!(rgb[0], 200, "R at (0,0)");
    }

    #[test]
    fn test_debayer_nn_bggr_2x2_b_at_origin() {
        // All-zero except B at (0,0) for BGGR = 200.
        let pixels = [200u8, 0, 0, 0]; // BGGR: (0,0)=B, (1,0)=G, (0,1)=G, (1,1)=R
        let rgb = debayer_nn(&pixels, 2, 2, BayerPattern::Bggr);
        // Pixel (0,0): B channel must be 200.
        assert_eq!(rgb[2], 200, "B at (0,0) for BGGR");
        // At the image corner, edge clamping can cause the diagonal R neighbours
        // to sample the B pixel itself, producing a small R bleed. The bleed
        // is well below the B value (200).
        assert!(
            rgb[0] < 100,
            "R bleed at corner should be well below B (got {})",
            rgb[0]
        );
    }

    #[test]
    fn test_debayer_nn_output_size() {
        let pixels = vec![128u8; 6 * 4]; // 6×4
        let rgb = debayer_nn(&pixels, 6, 4, BayerPattern::Rggb);
        assert_eq!(rgb.len(), 6 * 4 * 3);
    }
}
