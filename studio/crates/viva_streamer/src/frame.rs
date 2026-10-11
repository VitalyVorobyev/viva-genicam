//! Turning a camera frame into the BMP the viewer draws.
//!
//! One implementation for both of Studio's modes (backlog ST-32), and the same
//! one as the library: embedded mode passes the camera's PFNC format, remote
//! mode the wire's ([`viva_zenoh_api::PixelFormat::to_pfnc`]), and
//! [`FrameEncoder`] converts with [`viva_pfnc::convert`] — the conversion
//! `viva_genicam::Frame::to_rgb8` returns — so the viewer shows the pixels the
//! library hands an application.

use std::time::{Duration, Instant};

use bytes::Bytes;
use thiserror::Error;
use viva_pfnc::PixelFormat;
use viva_pfnc::convert::{self, ConvertError, Converter};

use crate::bmp::{BmpEncoder, BmpError};

/// Why a frame could not be drawn.
#[derive(Debug, Error)]
pub enum EncodeError {
    /// The pixels could not be converted: an undrawable format, or a payload
    /// shorter than its image.
    #[error(transparent)]
    Convert(#[from] ConvertError),
    #[error(transparent)]
    Bmp(#[from] BmpError),
}

/// Encodes frames as BMP, keeping its encoders and conversion buffers between
/// frames: only the BMP handed to the viewer is allocated per frame.
#[derive(Debug, Default)]
pub struct FrameEncoder {
    size: (u32, u32),
    gray: Option<BmpEncoder>,
    rgb: Option<BmpEncoder>,
    converter: Converter,
}

impl FrameEncoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether frames of `format` can be drawn.
    pub fn can_draw(format: PixelFormat) -> bool {
        convert::supports(format)
    }

    /// Encode one `width × height` frame: monochrome as an 8-bit grey BMP,
    /// everything else as 24-bit RGB. Bytes past the image are ignored.
    pub fn encode(
        &mut self,
        format: PixelFormat,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<Bytes, EncodeError> {
        if self.size != (width, height) {
            self.size = (width, height);
            self.gray = None;
            self.rgb = None;
        }
        if format.is_mono() {
            let gray = self.converter.gray8(format, width, height, pixels)?;
            let encoder = self
                .gray
                .get_or_insert_with(|| BmpEncoder::new(width, height));
            Ok(encoder.encode_gray8(gray)?)
        } else {
            let rgb = self.converter.rgb8(format, width, height, pixels)?;
            let encoder = self
                .rgb
                .get_or_insert_with(|| BmpEncoder::new_rgb24(width, height));
            Ok(encoder.encode_rgb24(rgb)?)
        }
    }
}

/// When to draw the next frame for the viewer; both modes ask it.
///
/// With a viewer connected, a frame is drawn at most once per `interval`.
/// With none, still once per [`Self::UNWATCHED_INTERVAL`]: the stored frame is
/// the first thing a viewer that connects later is sent, and in SingleFrame
/// mode it is the only frame there will be — the viewer connects after the
/// acquisition starts (Codex review of #173).
#[derive(Debug)]
pub struct FramePacer {
    interval: Duration,
    last: Option<Instant>,
}

impl FramePacer {
    /// How often the stored frame is refreshed while nobody watches.
    pub const UNWATCHED_INTERVAL: Duration = Duration::from_secs(1);

    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
        }
    }

    /// Whether a frame arriving at `now` should be drawn.
    pub fn is_due(&self, watched: bool, now: Instant) -> bool {
        let interval = if watched {
            self.interval
        } else {
            self.interval.max(Self::UNWATCHED_INTERVAL)
        };
        self.last
            .is_none_or(|last| now.saturating_duration_since(last) >= interval)
    }

    /// A frame was drawn at `now`.
    pub fn sent(&mut self, now: Instant) {
        self.last = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use viva_pfnc::PixelFormat;

    use super::{EncodeError, FrameEncoder, FramePacer};

    #[test]
    fn the_pacer_draws_the_first_frame_even_with_no_viewer() {
        let pacer = FramePacer::new(Duration::from_millis(33));
        assert!(pacer.is_due(false, Instant::now()));
        assert!(pacer.is_due(true, Instant::now()));
    }

    #[test]
    fn the_pacer_slows_to_once_a_second_while_nobody_watches() {
        let mut pacer = FramePacer::new(Duration::from_millis(33));
        let t0 = Instant::now();
        pacer.sent(t0);
        let later = t0 + Duration::from_millis(100);
        assert!(pacer.is_due(true, later), "a viewer gets the display rate");
        assert!(!pacer.is_due(false, later), "nobody watching: not yet");
        assert!(pacer.is_due(false, t0 + FramePacer::UNWATCHED_INTERVAL));
    }

    /// Pixel data offset and first pixel's bytes of a BMP.
    fn first_pixel(bmp: &[u8], bytes: usize) -> Vec<u8> {
        let offset = u32::from_le_bytes(bmp[10..14].try_into().unwrap()) as usize;
        bmp[offset..offset + bytes].to_vec()
    }

    #[test]
    fn mono12_is_drawn_from_its_top_eight_bits() {
        let mut enc = FrameEncoder::new();
        // 0x0FF0 at 12 bits is 0xFF at 8.
        let bmp = enc
            .encode(PixelFormat::Mono12, 1, 1, &[0xF0, 0x0F])
            .unwrap();
        assert_eq!(first_pixel(&bmp, 1), [0xFF]);
    }

    #[test]
    fn bgr8_lands_in_bmp_order() {
        let mut enc = FrameEncoder::new();
        let bmp = enc
            .encode(PixelFormat::BGR8Packed, 1, 1, &[1, 2, 3])
            .unwrap();
        assert_eq!(first_pixel(&bmp, 3), [1, 2, 3]);
        // Negative control: the same bytes as RGB are swapped.
        let bmp = enc
            .encode(PixelFormat::RGB8Packed, 1, 1, &[1, 2, 3])
            .unwrap();
        assert_eq!(first_pixel(&bmp, 3), [3, 2, 1]);
    }

    #[test]
    fn rgba8_drops_the_alpha() {
        let mut enc = FrameEncoder::new();
        let bmp = enc
            .encode(PixelFormat::RGBa8, 1, 1, &[1, 2, 3, 99])
            .unwrap();
        assert_eq!(first_pixel(&bmp, 3), [3, 2, 1]);
    }

    #[test]
    fn bayer12_is_downscaled_then_demosaiced() {
        let mut enc = FrameEncoder::new();
        // A uniform 2x2 mosaic at 12-bit 0x0800 is mid-grey everywhere.
        let mosaic: Vec<u8> = [0x00, 0x08].repeat(4);
        let bmp = enc.encode(PixelFormat::BayerRG12, 2, 2, &mosaic).unwrap();
        assert_eq!(first_pixel(&bmp, 3), [0x80, 0x80, 0x80]);
    }

    #[test]
    fn a_new_size_gets_new_headers() {
        let mut enc = FrameEncoder::new();
        let small = enc.encode(PixelFormat::Mono8, 2, 2, &[0; 4]).unwrap();
        let large = enc.encode(PixelFormat::Mono8, 4, 4, &[0; 16]).unwrap();
        let width = |bmp: &[u8]| i32::from_le_bytes(bmp[18..22].try_into().unwrap());
        assert_eq!((width(&small), width(&large)), (2, 4));
    }

    #[test]
    fn short_and_undrawable_frames_are_errors() {
        let mut enc = FrameEncoder::new();
        assert!(matches!(
            enc.encode(PixelFormat::Mono12Packed, 2, 2, &[0; 5]),
            Err(EncodeError::Convert(_))
        ));
        assert!(!FrameEncoder::can_draw(PixelFormat::Coord3DABC32f));
        assert!(FrameEncoder::can_draw(PixelFormat::YUV422_8_UYVY));
    }

    /// Trailing bytes (GVSP padding) are not part of the image.
    #[test]
    fn bytes_past_the_image_are_ignored() {
        let mut enc = FrameEncoder::new();
        let bmp = enc.encode(PixelFormat::Mono8, 1, 1, &[7, 0xEE]).unwrap();
        assert_eq!(first_pixel(&bmp, 1), [7]);
    }

    /// The pixels of a top-down BMP as RGB rows, grey or 24-bit alike.
    fn bmp_rgb(bmp: &[u8]) -> Vec<u8> {
        let u32_at = |i: usize| u32::from_le_bytes(bmp[i..i + 4].try_into().unwrap());
        let offset = u32_at(10) as usize;
        let width = u32_at(18) as usize;
        let height = (u32_at(22) as i32).unsigned_abs() as usize;
        let bits = u16::from_le_bytes([bmp[28], bmp[29]]);
        let bytes_per_pixel = usize::from(bits / 8);
        let stride = (width * bytes_per_pixel).div_ceil(4) * 4;
        let mut rgb = Vec::new();
        for y in 0..height {
            let row = &bmp[offset + y * stride..][..width * bytes_per_pixel];
            match bits {
                8 => rgb.extend(row.iter().flat_map(|&v| [v, v, v])),
                _ => rgb.extend(
                    row.as_chunks::<3>()
                        .0
                        .iter()
                        .flat_map(|[b, g, r]| [*r, *g, *b]),
                ),
            }
        }
        rgb
    }

    /// One implementation: what the viewer draws is, pixel for pixel, the RGB
    /// the library's `Frame::to_rgb8` returns — both are
    /// `viva_pfnc::convert::to_rgb8` — for every family of format the
    /// converter handles, packed and Y'CbCr included.
    #[test]
    fn the_viewer_draws_what_the_library_converts() {
        use viva_pfnc::PixelFormat as P;
        let (width, height) = (8, 4);
        for format in [
            P::Mono8,
            P::Mono10,
            P::Mono12,
            P::Mono14,
            P::Mono16,
            P::Mono10p,
            P::Mono12p,
            P::Mono10Packed,
            P::Mono12Packed,
            P::BayerRG8,
            P::BayerGR8,
            P::BayerGB8,
            P::BayerBG8,
            P::BayerRG10,
            P::BayerGR12,
            P::BayerBG16,
            P::BayerGB10p,
            P::BayerRG12p,
            P::BayerGR10Packed,
            P::BayerBG12Packed,
            P::RGB8Packed,
            P::BGR8Packed,
            P::RGBa8,
            P::BGRa8,
            P::YUV422_8,
            P::YUV422_8_UYVY,
            P::YCbCr422_8,
            P::YCbCr422_8_CbYCrY,
            P::YUV411_8_UYYVYY,
            P::YUV8_UYV,
            P::YCbCr8,
            P::YCbCr8_CbYCr,
        ] {
            let len = format.image_bytes(width, height).unwrap();
            // Deterministic bytes; 16-bit containers kept to `bits`.
            let mut src: Vec<u8> = (0..len).map(|i| (i * 89 + 7) as u8).collect();
            let unpacked_16 =
                format.bits_per_pixel() == Some(16) && format.bytes_per_pixel() == Some(2);
            if unpacked_16 && (format.is_mono() || format.is_bayer()) {
                // Mono12 → 12, BayerBG16 → 16.
                let name = format.to_string();
                let bits: u32 = name
                    .trim_start_matches(char::is_alphabetic)
                    .parse()
                    .unwrap();
                for pair in src.as_chunks_mut::<2>().0 {
                    *pair = (u16::from_le_bytes(*pair) >> (16 - bits)).to_le_bytes();
                }
            }
            let library = viva_pfnc::convert::to_rgb8(format, width, height, &src).unwrap();
            let bmp = FrameEncoder::new()
                .encode(format, width, height, &src)
                .unwrap_or_else(|e| panic!("{format}: {e}"));
            assert_eq!(bmp_rgb(&bmp), library, "{format}");
        }
    }
}
