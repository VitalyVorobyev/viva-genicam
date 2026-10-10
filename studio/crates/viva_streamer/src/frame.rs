//! Turning a camera frame into the BMP the viewer draws.
//!
//! One implementation for both of Studio's modes (backlog ST-32): embedded
//! mode maps the camera's PFNC format to a [`PixelLayout`], remote mode maps
//! the wire's, and [`FrameEncoder`] does the rest. Each mode used to carry its
//! own encoder, and the embedded one could not show Mono10/12, Bayer above 8
//! bits or RGBa8.

use std::time::{Duration, Instant};

use bytes::Bytes;
use viva_zenoh_api::PixelFormat as WirePixelFormat;

use crate::bmp::{
    BayerPattern, BmpEncoder, BmpError, debayer_bilinear_into, mono_u16le_to_gray8_into,
};

/// How a frame's pixels are laid out, as far as drawing them is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelLayout {
    /// One sample per pixel: one byte for 8 bits, two little-endian bytes
    /// above that.
    Mono {
        bits: u8,
    },
    /// A Bayer mosaic, sampled as [`PixelLayout::Mono`] is.
    Bayer {
        pattern: BayerPattern,
        bits: u8,
    },
    Rgb8,
    Bgr8,
    /// RGB with an alpha byte, which is dropped.
    Rgba8,
}

impl PixelLayout {
    /// The layout of a wire format, or `None` for one the viewer cannot draw.
    pub fn of_wire(format: &WirePixelFormat) -> Option<Self> {
        use BayerPattern::{Bggr, Gbrg, Grbg, Rggb};
        use WirePixelFormat as F;
        let bayer = |pattern, bits| Some(Self::Bayer { pattern, bits });
        match format {
            F::Mono8 => Some(Self::Mono { bits: 8 }),
            F::Mono10 => Some(Self::Mono { bits: 10 }),
            F::Mono12 => Some(Self::Mono { bits: 12 }),
            F::Mono16 => Some(Self::Mono { bits: 16 }),
            F::BayerRG8 => bayer(Rggb, 8),
            F::BayerGR8 => bayer(Grbg, 8),
            F::BayerBG8 => bayer(Bggr, 8),
            F::BayerGB8 => bayer(Gbrg, 8),
            F::BayerRG10 => bayer(Rggb, 10),
            F::BayerGR10 => bayer(Grbg, 10),
            F::BayerBG10 => bayer(Bggr, 10),
            F::BayerGB10 => bayer(Gbrg, 10),
            F::BayerRG12 => bayer(Rggb, 12),
            F::BayerGR12 => bayer(Grbg, 12),
            F::BayerBG12 => bayer(Bggr, 12),
            F::BayerGB12 => bayer(Gbrg, 12),
            F::BayerRG16 => bayer(Rggb, 16),
            F::BayerGR16 => bayer(Grbg, 16),
            F::BayerBG16 => bayer(Bggr, 16),
            F::BayerGB16 => bayer(Gbrg, 16),
            F::RGB8 => Some(Self::Rgb8),
            F::BGR8 => Some(Self::Bgr8),
            F::RGBa8 => Some(Self::Rgba8),
            _ => None,
        }
    }
}

/// Encodes frames as BMP, keeping its encoders and scratch buffers between
/// frames: only the BMP handed to the viewer is allocated per frame.
#[derive(Debug, Default)]
pub struct FrameEncoder {
    size: (u32, u32),
    gray: Option<BmpEncoder>,
    rgb: Option<BmpEncoder>,
    samples: Vec<u8>,
    colour: Vec<u8>,
}

impl FrameEncoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Encode one `width × height` frame of `layout`.
    pub fn encode(
        &mut self,
        layout: PixelLayout,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<Bytes, BmpError> {
        if self.size != (width, height) {
            self.size = (width, height);
            self.gray = None;
            self.rgb = None;
        }
        match layout {
            PixelLayout::Mono { bits: 8 } => self.gray().encode_gray8(pixels),
            PixelLayout::Mono { bits } => {
                mono_u16le_to_gray8_into(pixels, bits, &mut self.samples);
                let samples = std::mem::take(&mut self.samples);
                let bmp = self.gray().encode_gray8(&samples);
                self.samples = samples;
                bmp
            }
            PixelLayout::Bayer { pattern, bits } => {
                let mosaic: &[u8] = if bits == 8 {
                    pixels
                } else {
                    mono_u16le_to_gray8_into(pixels, bits, &mut self.samples);
                    &self.samples
                };
                debayer_bilinear_into(mosaic, width, height, pattern, &mut self.colour);
                let colour = std::mem::take(&mut self.colour);
                let bmp = self.rgb().encode_rgb24(&colour);
                self.colour = colour;
                bmp
            }
            PixelLayout::Rgb8 => self.rgb().encode_rgb24(pixels),
            PixelLayout::Bgr8 => self.rgb().encode_bgr24(pixels),
            PixelLayout::Rgba8 => {
                self.colour.clear();
                self.colour.extend(
                    pixels
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .flat_map(|[r, g, b, _]| [*r, *g, *b]),
                );
                let colour = std::mem::take(&mut self.colour);
                let bmp = self.rgb().encode_rgb24(&colour);
                self.colour = colour;
                bmp
            }
        }
    }

    fn gray(&mut self) -> &BmpEncoder {
        let (w, h) = self.size;
        self.gray.get_or_insert_with(|| BmpEncoder::new(w, h))
    }

    fn rgb(&mut self) -> &BmpEncoder {
        let (w, h) = self.size;
        self.rgb.get_or_insert_with(|| BmpEncoder::new_rgb24(w, h))
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

    use super::{FrameEncoder, FramePacer, PixelLayout};
    use crate::bmp::BayerPattern;

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
            .encode(PixelLayout::Mono { bits: 12 }, 1, 1, &[0xF0, 0x0F])
            .unwrap();
        assert_eq!(first_pixel(&bmp, 1), [0xFF]);
    }

    #[test]
    fn bgr8_lands_in_bmp_order_untouched() {
        let mut enc = FrameEncoder::new();
        let bmp = enc.encode(PixelLayout::Bgr8, 1, 1, &[1, 2, 3]).unwrap();
        assert_eq!(first_pixel(&bmp, 3), [1, 2, 3]);
        // Negative control: the same bytes as RGB are swapped.
        let bmp = enc.encode(PixelLayout::Rgb8, 1, 1, &[1, 2, 3]).unwrap();
        assert_eq!(first_pixel(&bmp, 3), [3, 2, 1]);
    }

    #[test]
    fn rgba8_drops_the_alpha() {
        let mut enc = FrameEncoder::new();
        let bmp = enc
            .encode(PixelLayout::Rgba8, 1, 1, &[1, 2, 3, 99])
            .unwrap();
        assert_eq!(first_pixel(&bmp, 3), [3, 2, 1]);
    }

    #[test]
    fn bayer12_is_downscaled_then_demosaiced() {
        let mut enc = FrameEncoder::new();
        // A uniform 2x2 mosaic at 12-bit 0x0800 is mid-grey everywhere.
        let mosaic: Vec<u8> = [0x00, 0x08].repeat(4);
        let layout = PixelLayout::Bayer {
            pattern: BayerPattern::Rggb,
            bits: 12,
        };
        let bmp = enc.encode(layout, 2, 2, &mosaic).unwrap();
        assert_eq!(first_pixel(&bmp, 3), [0x80, 0x80, 0x80]);
    }

    #[test]
    fn a_new_size_gets_new_headers() {
        let mut enc = FrameEncoder::new();
        let small = enc
            .encode(PixelLayout::Mono { bits: 8 }, 2, 2, &[0; 4])
            .unwrap();
        let large = enc
            .encode(PixelLayout::Mono { bits: 8 }, 4, 4, &[0; 16])
            .unwrap();
        let width = |bmp: &[u8]| i32::from_le_bytes(bmp[18..22].try_into().unwrap());
        assert_eq!((width(&small), width(&large)), (2, 4));
    }
}
