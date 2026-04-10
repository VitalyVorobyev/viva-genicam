//! Frame representation combining pixel data with optional chunk metadata.

use std::time::SystemTime;

use bytes::Bytes;
use tracing::debug;
use viva_pfnc::PixelFormat;

use crate::chunks::{ChunkKind, ChunkMap, ChunkValue};

/// Image frame produced by the GigE Vision stream reassembler.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Contiguous image payload containing pixel data.
    pub payload: Bytes,
    /// Width of the image in pixels.
    pub width: u32,
    /// Height of the image in pixels.
    pub height: u32,
    /// Pixel format describing how to interpret the payload bytes.
    pub pixel_format: PixelFormat,
    /// Optional map of chunk values decoded from the GVSP trailer.
    pub chunks: Option<ChunkMap>,
    /// Device timestamp reported by the camera when available.
    pub ts_dev: Option<u64>,
    /// Host timestamp obtained by mapping the device ticks.
    pub ts_host: Option<SystemTime>,
}

impl Frame {
    /// Retrieve a chunk value by kind if it exists.
    pub fn chunk(&self, kind: ChunkKind) -> Option<&ChunkValue> {
        self.chunks.as_ref()?.get(&kind)
    }

    /// Host-reconstructed timestamp if the camera reports a device timestamp.
    pub fn host_time(&self) -> Option<SystemTime> {
        self.ts_host
    }

    /// Return a borrowed slice of RGB pixels when the payload is already RGB8.
    pub fn as_rgb8(&self) -> Option<&[u8]> {
        match self.pixel_format {
            PixelFormat::RGB8Packed => Some(self.payload.as_ref()),
            _ => None,
        }
    }

    /// Convert the frame payload into an owned RGB8 buffer.
    pub fn to_rgb8(&self) -> Result<Vec<u8>, crate::GenicamError> {
        if let Some(rgb) = self.as_rgb8() {
            return Ok(rgb.to_vec());
        }

        match self.pixel_format {
            PixelFormat::Mono8 => self.mono8_to_rgb8(),
            PixelFormat::Mono16 => self.mono16_to_rgb8(),
            PixelFormat::BGR8Packed => self.bgr8_to_rgb8(),
            PixelFormat::BayerRG8
            | PixelFormat::BayerGB8
            | PixelFormat::BayerBG8
            | PixelFormat::BayerGR8 => self.bayer_to_rgb8(),
            PixelFormat::RGB8Packed => unreachable!("handled by as_rgb8 fast path"),
            _ => Err(crate::GenicamError::UnsupportedPixelFormat(
                self.pixel_format,
            )),
        }
    }

    fn total_pixels(&self) -> Result<usize, crate::GenicamError> {
        let width = usize::try_from(self.width)
            .map_err(|_| crate::GenicamError::Parse("frame width exceeds address space".into()))?;
        let height = usize::try_from(self.height)
            .map_err(|_| crate::GenicamError::Parse("frame height exceeds address space".into()))?;
        width
            .checked_mul(height)
            .ok_or_else(|| crate::GenicamError::Parse("frame dimensions overflow".into()))
    }

    fn expect_payload_len(&self, expected: usize, fmt: &str) -> Result<(), crate::GenicamError> {
        if self.payload.len() != expected {
            return Err(crate::GenicamError::Parse(format!(
                "payload length {} does not match {} expectation {}",
                self.payload.len(),
                fmt,
                expected
            )));
        }
        Ok(())
    }

    fn mono8_to_rgb8(&self) -> Result<Vec<u8>, crate::GenicamError> {
        let pixels = self.total_pixels()?;
        self.expect_payload_len(pixels, "Mono8")?;
        debug!(
            width = self.width,
            height = self.height,
            "converting Mono8 frame to RGB8"
        );
        let mut out = Vec::with_capacity(pixels * 3);
        for &value in self.payload.as_ref() {
            out.extend_from_slice(&[value, value, value]);
        }
        Ok(out)
    }

    fn mono16_to_rgb8(&self) -> Result<Vec<u8>, crate::GenicamError> {
        let pixels = self.total_pixels()?;
        let expected = pixels
            .checked_mul(2)
            .ok_or_else(|| crate::GenicamError::Parse("Mono16 payload overflow".into()))?;
        self.expect_payload_len(expected, "Mono16")?;
        debug!(
            width = self.width,
            height = self.height,
            "converting Mono16 frame to RGB8"
        );
        let mut out = Vec::with_capacity(pixels * 3);
        let data = self.payload.as_ref();
        for idx in 0..pixels {
            let hi = data[idx * 2 + 1];
            out.extend_from_slice(&[hi, hi, hi]);
        }
        Ok(out)
    }

    fn bgr8_to_rgb8(&self) -> Result<Vec<u8>, crate::GenicamError> {
        let pixels = self.total_pixels()?;
        let expected = pixels
            .checked_mul(3)
            .ok_or_else(|| crate::GenicamError::Parse("BGR8 payload overflow".into()))?;
        self.expect_payload_len(expected, "BGR8")?;
        debug!(
            width = self.width,
            height = self.height,
            "converting BGR8 frame to RGB8"
        );
        let mut out = Vec::with_capacity(expected);
        for chunk in self.payload.chunks_exact(3) {
            out.extend_from_slice(&[chunk[2], chunk[1], chunk[0]]);
        }
        Ok(out)
    }

    fn bayer_to_rgb8(&self) -> Result<Vec<u8>, crate::GenicamError> {
        let pixels = self.total_pixels()?;
        self.expect_payload_len(pixels, "Bayer8")?;
        let (pattern, x_offset, y_offset) =
            self.pixel_format
                .cfa_pattern()
                .ok_or(crate::GenicamError::UnsupportedPixelFormat(
                    self.pixel_format,
                ))?;
        debug!(
            width = self.width,
            height = self.height,
            pattern,
            x_offset,
            y_offset,
            "demosaicing Bayer frame"
        );

        let width = usize::try_from(self.width)
            .map_err(|_| crate::GenicamError::Parse("frame width exceeds address space".into()))?;
        let height = usize::try_from(self.height)
            .map_err(|_| crate::GenicamError::Parse("frame height exceeds address space".into()))?;
        let src = self.payload.as_ref();
        let mut out = vec![0u8; width * height * 3];

        for y in 0..height {
            for x in 0..width {
                let dst_idx = (y * width + x) * 3;
                let (r, g, b) = demosaic_pixel(src, width, height, x, y, x_offset, y_offset);
                out[dst_idx] = r;
                out[dst_idx + 1] = g;
                out[dst_idx + 2] = b;
            }
        }

        Ok(out)
    }
}

fn demosaic_pixel(
    src: &[u8],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    x_offset: u8,
    y_offset: u8,
) -> (u8, u8, u8) {
    use core::cmp::min;

    let clamp = |value: isize, upper: usize| -> usize {
        if value < 0 {
            0
        } else {
            min(value as usize, upper.saturating_sub(1))
        }
    };

    let sample = |sx: isize, sy: isize| -> u8 {
        let cx = clamp(sx, width);
        let cy = clamp(sy, height);
        src[cy * width + cx]
    };

    let x = x as isize;
    let y = y as isize;
    let ox = x_offset as isize;
    let oy = y_offset as isize;
    let mx = ((x + ox) & 1) as i32;
    let my = ((y + oy) & 1) as i32;

    match (mx, my) {
        (0, 0) => {
            let r = sample(x, y);
            let g1 = sample(x + 1, y);
            let g2 = sample(x, y + 1);
            let b = sample(x + 1, y + 1);
            let g = ((g1 as u16 + g2 as u16) / 2) as u8;
            (r, g, b)
        }
        (1, 1) => {
            let r = sample(x - 1, y - 1);
            let g1 = sample(x - 1, y);
            let g2 = sample(x, y - 1);
            let b = sample(x, y);
            let g = ((g1 as u16 + g2 as u16) / 2) as u8;
            (r, g, b)
        }
        (1, 0) => {
            let r = sample(x - 1, y);
            let g = sample(x, y);
            let b = sample(x, y + 1);
            (r, g, b)
        }
        (0, 1) => {
            let r = sample(x, y - 1);
            let g = sample(x, y);
            let b = sample(x + 1, y);
            (r, g, b)
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn frame_with_payload(
        payload: &[u8],
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
    ) -> Frame {
        Frame {
            payload: Bytes::copy_from_slice(payload),
            width,
            height,
            pixel_format,
            chunks: None,
            ts_dev: None,
            ts_host: None,
        }
    }

    #[test]
    fn mono8_converts_to_rgb8() {
        let payload = [0u8, 64, 128, 255];
        let frame = frame_with_payload(&payload, 2, 2, PixelFormat::Mono8);
        let rgb = frame.to_rgb8().expect("mono conversion");
        assert_eq!(rgb.len(), 12);
        assert_eq!(&rgb[0..3], &[0, 0, 0]);
        assert_eq!(&rgb[3..6], &[64, 64, 64]);
        assert_eq!(&rgb[9..12], &[255, 255, 255]);
    }

    #[test]
    fn rgb8_fast_path_borrows_payload() {
        let payload = vec![1u8, 2, 3, 4, 5, 6];
        let frame = frame_with_payload(&payload, 1, 2, PixelFormat::RGB8Packed);
        assert_eq!(frame.as_rgb8().unwrap(), payload.as_slice());
        let owned = frame.to_rgb8().expect("rgb copy");
        assert_eq!(owned, payload);
    }

    #[test]
    fn bayer_rg8_demosaic_basic_pattern() {
        // Simple 4x4 Bayer pattern with distinguishable colour quadrants.
        let payload = [
            255, 32, 255, 32, // R G R G row
            32, 16, 32, 240, // G B G B row
            255, 32, 255, 32, 32, 16, 32, 240,
        ];
        let frame = frame_with_payload(&payload, 4, 4, PixelFormat::BayerRG8);
        let rgb = frame.to_rgb8().expect("bayer conversion");
        assert_eq!(rgb.len(), 4 * 4 * 3);
        // Top-left pixel should be red dominant.
        assert!(rgb[0] > rgb[1] && rgb[0] > rgb[2]);
        // Bottom-right pixel should carry the blue sample.
        let last = &rgb[rgb.len() - 3..];
        assert_eq!(last[2], 240);
    }
}
