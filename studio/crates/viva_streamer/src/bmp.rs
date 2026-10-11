//! BMP encoder for 8-bit grayscale (Mono8) and 24-bit RGB (colour formats).
//!
//! Two encoder modes:
//!
//! - **8bpp grayscale** (`BmpEncoder::new`): BITMAPFILEHEADER + BITMAPINFOHEADER +
//!   256-entry grayscale palette + 1-byte-per-pixel data with 4-byte row alignment.
//!   Used for monochrome formats, normalised to 8 bits.
//!
//! - **24bpp RGB** (`BmpEncoder::new_rgb24`): BITMAPFILEHEADER + BITMAPINFOHEADER +
//!   no palette + 3-bytes-per-pixel BGR data with 4-byte row alignment.
//!   Used for every colour format, converted to RGB8.
//!
//! Both modes use a *top-down* DIB (negative height in the header) so rows are
//! written in top-first order without flipping.
//!
//! The pixels come from `viva_pfnc::convert`, which [`crate::frame::FrameEncoder`]
//! runs on a camera frame first.

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

        // One allocation, rows written in place; padding stays zero.
        let mut buffer = vec![0u8; self.header_prefix.len() + self.image_size];
        buffer[..self.header_prefix.len()].copy_from_slice(&self.header_prefix);
        let row_width_bytes = self.width as usize * 3;
        let image = &mut buffer[self.header_prefix.len()..];
        if row_width_bytes > 0 {
            for (row, src) in pixels.chunks_exact(row_width_bytes).enumerate() {
                let dst =
                    &mut image[row * self.row_stride..row * self.row_stride + row_width_bytes];
                let src = src.as_chunks::<3>().0;
                for (d, [r, g, b]) in dst.as_chunks_mut::<3>().0.iter_mut().zip(src) {
                    *d = [*b, *g, *r];
                }
            }
        }

        Ok(Bytes::from(buffer))
    }
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
}
