//! Turning a frame's bytes into samples: unpack, normalise, demosaic,
//! colour-convert.
//!
//! The one implementation of pixel conversion in the project: the library's
//! `Frame::to_rgb8` and image writer, and Viva Studio's viewer, all call it.
//! No I/O, no dependencies; it compiles for `wasm32-unknown-unknown`.
//!
//! # What each step does
//!
//! - **Unpack** ([`unpack_u16`]) reads a single-component format — `Mono*`
//!   or `Bayer*` — into one `u16` per pixel holding the camera's value:
//!   lsb-unpacked formats from their little-endian bytes (PFNC 2.4 §6.1.1),
//!   `10p`/`12p` as an lsb-first bit stream (§6.3.1), and the GigE Vision 2.0
//!   `10Packed`/`12Packed` formats from their two-pixels-in-three-bytes layout
//!   (see [`PixelFormat::Mono12Packed`]).
//! - **Normalise to 8 bits** keeps a sample's top eight bits: `v >> (bits − 8)`.
//! - **Demosaic** is bilinear: each missing colour is the truncated integer
//!   mean of the two or four nearest samples of that colour, and a neighbour
//!   outside the image is replaced by the nearest sample inside it
//!   (clamp-to-edge), so the outermost row and column mix in the wrong colour.
//!   It is the kernel Viva Studio's viewer used before this module, a preview
//!   quality demosaic, not an ISP. To 8 bits, the mosaic is normalised first
//!   and then demosaiced; at full depth it is demosaiced at its own depth.
//! - **Y'CbCr to RGB** uses the full-range equations of PFNC 2.4 §8.2.1,
//!   rounded and clamped to 0-255. PFNC calls the legacy `YUV` formats
//!   Y'CbCr with U and V offset by 128 (Table 3-1) and gives them no
//!   equations of their own, so they get the same ones. A 4:2:2 or 4:1:1
//!   pixel uses the Cb/Cr of its group.
//! - **RGB family**: `BGR` is reordered and an alpha byte dropped.
//!
//! ```
//! use viva_pfnc::PixelFormat;
//! use viva_pfnc::convert::{self, Converter};
//!
//! // Two 12-bit pixels, 0xABC and 0x123, in the GigE Vision layout.
//! let bytes = [0xAB, 0x3C, 0x12];
//! let samples = convert::unpack_u16(PixelFormat::Mono12Packed, 2, 1, None, &bytes)?;
//! assert_eq!(samples, [0xABC, 0x123]);
//!
//! // For display: the top eight bits, through buffers kept between frames.
//! let mut converter = Converter::new();
//! let grey = converter.gray8(PixelFormat::Mono12Packed, 2, 1, None, &bytes)?;
//! assert_eq!(grey, [0xAB, 0x12]);
//! # Ok::<(), convert::ConvertError>(())
//! ```
//!
//! # Geometry
//!
//! Without a row stride, pixels are read as one continuous run from the
//! first line to the last — PFNC 2.4 §6.7's *image padding*. A bit-packed
//! payload (`…p`, `…Packed`) must hold every pixel's bits,
//! ⌈pixels × bits / 8⌉, and any other at least
//! [`PixelFormat::image_bytes`]; bytes past that are ignored. §6.7 completes
//! the packing group the image ends inside, but an Allied Vision Manta
//! G-125B does not: at 1291 × 963 in `Mono12Packed`, an odd pixel count, it
//! sends ⌈pixels × 12 / 8⌉ bytes, one short of the completed group. A payload
//! with the group completed decodes the same.
//!
//! With a row stride — a camera that sends bytes after every line, as the
//! GVSP image leader's `padding_x` declares — each line is decoded on its
//! own: [`PixelFormat::line_bytes`] of pixels, a packing group the line ends
//! inside completed (§6.7 *line padding*), then the next line `row_stride`
//! bytes after the start of this one.
//!
//! The two readings differ only when a line ends inside a packing group (an
//! odd width in `12p`, say). PFNC leaves the choice to the transport
//! standard. The Manta G-125B declares no padding at an odd width in
//! `Mono12Packed` and runs its pixels on, which is the first reading; how a
//! camera that declares `padding_x` counts the group its line ends inside is
//! open (backlog PX-05).

use core::fmt;

use crate::PixelFormat;

/// Why a frame could not be converted.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConvertError {
    /// The format has no conversion here (`Coord3D_*`, `Confidence8`,
    /// event streams, codes this crate does not name).
    Unsupported(PixelFormat),
    /// The payload is shorter than the image it claims to be.
    ShortPayload {
        /// The frame's pixel format.
        format: PixelFormat,
        /// Bytes the geometry needs ([`PixelFormat::image_bytes`]).
        expected: usize,
        /// Bytes the payload has.
        actual: usize,
    },
    /// The row stride is shorter than one line of the image.
    StrideTooShort {
        /// Bytes from one line to the next, as given.
        stride: usize,
        /// Bytes one line needs ([`PixelFormat::line_bytes`]).
        line: usize,
    },
    /// `width × height` does not fit in memory.
    TooLarge {
        /// Frame width in pixels.
        width: u32,
        /// Frame height in pixels.
        height: u32,
    },
}

impl fmt::Display for ConvertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConvertError::Unsupported(format) => {
                write!(f, "no conversion for pixel format {format}")
            }
            ConvertError::ShortPayload {
                format,
                expected,
                actual,
            } => write!(
                f,
                "{format} payload is {actual} bytes; the image needs {expected}"
            ),
            ConvertError::StrideTooShort { stride, line } => write!(
                f,
                "a row stride of {stride} bytes is shorter than a line, {line} bytes"
            ),
            ConvertError::TooLarge { width, height } => {
                write!(f, "a {width}×{height} image does not fit in memory")
            }
        }
    }
}

impl std::error::Error for ConvertError {}

/// An image at the camera's own depth, as [`full_depth`] returns it.
///
/// 16-bit samples hold the camera's values, `0 ..= 2^bits − 1`, not scaled
/// to 16 bits. Colour samples are interleaved R, G, B.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Pixels {
    /// One 8-bit sample per pixel.
    Gray8(Vec<u8>),
    /// One sample per pixel, `bits` of it significant.
    Gray16 {
        /// The camera's values.
        samples: Vec<u16>,
        /// The camera's bit depth, 9 to 16.
        bits: u8,
    },
    /// Three 8-bit samples per pixel.
    Rgb8(Vec<u8>),
    /// Three samples per pixel, `bits` of each significant.
    Rgb16 {
        /// Demosaiced values at the camera's depth.
        samples: Vec<u16>,
        /// The camera's bit depth, 9 to 16.
        bits: u8,
    },
}

impl Pixels {
    /// The same image in colour: grey samples are repeated into R, G and B;
    /// colour is returned as it is.
    pub fn into_rgb(self) -> Pixels {
        fn triple<T: Copy>(gray: &[T]) -> Vec<T> {
            gray.iter().flat_map(|&v| [v, v, v]).collect()
        }
        match self {
            Pixels::Gray8(gray) => Pixels::Rgb8(triple(&gray)),
            Pixels::Gray16 { samples, bits } => Pixels::Rgb16 {
                samples: triple(&samples),
                bits,
            },
            colour => colour,
        }
    }
}

/// Whether [`to_rgb8`] converts `format` — the formats a viewer can draw.
pub fn supports(format: PixelFormat) -> bool {
    layout(format).is_some()
}

/// The camera's sample values of a single-component format (`Mono*`,
/// `Bayer*`), one per pixel, row-major. A Bayer mosaic is returned as it is.
///
/// `row_stride` is the distance in bytes from the start of one line to the
/// next when the camera pads its lines; `None` reads the image as one run.
/// The module's *Geometry* section says which is which.
pub fn unpack_u16(
    format: PixelFormat,
    width: u32,
    height: u32,
    row_stride: Option<usize>,
    src: &[u8],
) -> Result<Vec<u16>, ConvertError> {
    let rows = rows(format, width, height, row_stride, src)?;
    let single = match layout(format) {
        Some(Layout::Mono(single) | Layout::Bayer(single, _)) => single,
        _ => return Err(ConvertError::Unsupported(format)),
    };
    let mut out = Vec::new();
    unpack_rows(single, rows, &mut out);
    Ok(out)
}

/// The frame as 8-bit RGB, three bytes per pixel.
///
/// Allocates its output; [`Converter::rgb8`] reuses buffers between frames.
pub fn to_rgb8(
    format: PixelFormat,
    width: u32,
    height: u32,
    row_stride: Option<usize>,
    src: &[u8],
) -> Result<Vec<u8>, ConvertError> {
    let mut out = Vec::new();
    Converter::new().rgb8_into(format, width, height, row_stride, src, &mut out)?;
    Ok(out)
}

/// The frame without losing depth: monochrome as grey, everything else as
/// RGB, 16-bit when the camera's samples are wider than 8 bits — a Bayer
/// mosaic demosaiced at its own depth.
pub fn full_depth(
    format: PixelFormat,
    width: u32,
    height: u32,
    row_stride: Option<usize>,
    src: &[u8],
) -> Result<Pixels, ConvertError> {
    let layout = layout(format).ok_or(ConvertError::Unsupported(format))?;
    let rows = rows(format, width, height, row_stride, src)?;
    let mut conv = Converter::new();
    Ok(match layout {
        Layout::Mono(single) if single.is_8_bit() => {
            let mut scratch = Vec::new();
            Pixels::Gray8(gather8(rows, &mut scratch).to_vec())
        }
        Layout::Mono(single) => {
            let mut samples = Vec::new();
            unpack_rows(single, rows, &mut samples);
            Pixels::Gray16 {
                samples,
                bits: single.bits,
            }
        }
        Layout::Bayer(single, offsets) if !single.is_8_bit() => {
            unpack_rows(single, rows, &mut conv.samples);
            let mut samples = Vec::new();
            demosaic_bilinear(
                &conv.samples,
                width as usize,
                height as usize,
                offsets,
                &mut samples,
            );
            Pixels::Rgb16 {
                samples,
                bits: single.bits,
            }
        }
        _ => {
            let mut out = Vec::new();
            conv.rgb8_into(format, width, height, row_stride, src, &mut out)?;
            Pixels::Rgb8(out)
        }
    })
}

/// Converts frames, keeping its buffers between them: after the first frame
/// of a size, converting another allocates nothing.
#[derive(Debug, Default, Clone)]
pub struct Converter {
    /// Unpacked samples.
    samples: Vec<u16>,
    /// An unpacked mosaic normalised to 8 bits.
    mosaic: Vec<u8>,
    /// The result handed out.
    out: Vec<u8>,
}

impl Converter {
    /// A converter with empty buffers.
    pub fn new() -> Self {
        Self::default()
    }

    /// A monochrome frame as 8-bit grey, one byte per pixel; `Mono8` without
    /// line padding is lent back as it is. [`ConvertError::Unsupported`] for
    /// anything else.
    pub fn gray8<'a>(
        &'a mut self,
        format: PixelFormat,
        width: u32,
        height: u32,
        row_stride: Option<usize>,
        src: &'a [u8],
    ) -> Result<&'a [u8], ConvertError> {
        let Some(Layout::Mono(single)) = layout(format) else {
            return Err(ConvertError::Unsupported(format));
        };
        let rows = rows(format, width, height, row_stride, src)?;
        if single.is_8_bit() {
            return Ok(gather8(rows, &mut self.out));
        }
        unpack_rows(single, rows, &mut self.samples);
        narrow_into(&self.samples, single.bits, &mut self.out);
        Ok(&self.out)
    }

    /// The frame as 8-bit RGB, three bytes per pixel; `RGB8` without line
    /// padding is lent back as it is.
    pub fn rgb8<'a>(
        &'a mut self,
        format: PixelFormat,
        width: u32,
        height: u32,
        row_stride: Option<usize>,
        src: &'a [u8],
    ) -> Result<&'a [u8], ConvertError> {
        if format == PixelFormat::RGB8Packed {
            let rows = rows(format, width, height, row_stride, src)?;
            if rows.count <= 1 {
                return Ok(&rows.src[..rows.pixels * 3]);
            }
        }
        let mut out = core::mem::take(&mut self.out);
        let result = self.rgb8_into(format, width, height, row_stride, src, &mut out);
        self.out = out;
        result.map(|()| self.out.as_slice())
    }

    fn rgb8_into(
        &mut self,
        format: PixelFormat,
        width: u32,
        height: u32,
        row_stride: Option<usize>,
        src: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), ConvertError> {
        let layout = layout(format).ok_or(ConvertError::Unsupported(format))?;
        let rows = rows(format, width, height, row_stride, src)?;
        out.clear();
        match layout {
            Layout::Mono(single) => {
                if single.is_8_bit() {
                    for row in rows.iter() {
                        out.extend(row[..rows.pixels].iter().flat_map(|&v| [v, v, v]));
                    }
                } else {
                    unpack_rows(single, rows, &mut self.samples);
                    let bits = single.bits;
                    out.extend(self.samples.iter().flat_map(|&v| {
                        let v = narrow(v, bits);
                        [v, v, v]
                    }));
                }
            }
            Layout::Bayer(single, offsets) => {
                let mosaic = if single.is_8_bit() {
                    gather8(rows, &mut self.mosaic)
                } else {
                    unpack_rows(single, rows, &mut self.samples);
                    narrow_into(&self.samples, single.bits, &mut self.mosaic);
                    &self.mosaic[..]
                };
                demosaic_bilinear(mosaic, width as usize, height as usize, offsets, out);
            }
            Layout::Rgb(order) => {
                let stride = order.stride();
                let [r, g, b] = order.indices();
                for row in rows.iter() {
                    out.extend(
                        row.chunks_exact(stride)
                            .take(rows.pixels)
                            .flat_map(|px| [px[r], px[g], px[b]]),
                    );
                }
            }
            Layout::Yuv(order) => {
                out.reserve(rows.total() * 3);
                for row in rows.iter() {
                    yuv_append(order, rows.pixels, row, out);
                }
            }
        }
        Ok(())
    }
}

/// Where an image's pixels lie in its payload: one run, or one line every
/// `stride` bytes.
#[derive(Clone, Copy)]
struct Rows<'a> {
    src: &'a [u8],
    /// Pixels in a row: the whole image when it is one run.
    pixels: usize,
    /// Bytes holding a row's pixels.
    bytes: usize,
    stride: usize,
    count: usize,
}

impl<'a> Rows<'a> {
    fn iter(self) -> impl Iterator<Item = &'a [u8]> {
        (0..self.count).map(move |y| &self.src[y * self.stride..][..self.bytes])
    }

    fn total(self) -> usize {
        self.pixels * self.count
    }
}

/// The rows of an image, or why its payload cannot hold them.
///
/// Without a stride the image is one run of `width × height` pixels (PFNC
/// 2.4 §6.7 image padding). With one, each line holds `width` pixels, a
/// packing group it ends inside completed (§6.7 line padding), and starts
/// `stride` bytes after the previous one; the last needs no padding after it.
fn rows(
    format: PixelFormat,
    width: u32,
    height: u32,
    row_stride: Option<usize>,
    src: &[u8],
) -> Result<Rows<'_>, ConvertError> {
    let Some(stride) = row_stride else {
        let (pixels, src) = geometry(format, width, height, src)?;
        return Ok(Rows {
            src,
            pixels,
            bytes: src.len(),
            stride: src.len(),
            count: 1,
        });
    };
    let too_large = ConvertError::TooLarge { width, height };
    (width as usize)
        .checked_mul(height as usize)
        .ok_or(too_large.clone())?;
    let line = format.line_bytes(width).ok_or(too_large.clone())?;
    if stride < line {
        return Err(ConvertError::StrideTooShort { stride, line });
    }
    let expected = match (height as usize).checked_sub(1) {
        None => 0,
        Some(before_last) => stride
            .checked_mul(before_last)
            .and_then(|start| start.checked_add(line))
            .ok_or(too_large)?,
    };
    if src.len() < expected {
        return Err(ConvertError::ShortPayload {
            format,
            expected,
            actual: src.len(),
        });
    }
    Ok(Rows {
        src,
        pixels: width as usize,
        bytes: line,
        stride,
        count: height as usize,
    })
}

/// The 8-bit samples of `rows` in one slice: borrowed when the image is one
/// run, gathered into `scratch` otherwise.
fn gather8<'a>(rows: Rows<'a>, scratch: &'a mut Vec<u8>) -> &'a [u8] {
    if rows.count <= 1 {
        return &rows.src[..rows.total()];
    }
    scratch.clear();
    for row in rows.iter() {
        scratch.extend_from_slice(&row[..rows.pixels]);
    }
    scratch
}

/// Every row's samples, in order.
fn unpack_rows(single: Single, rows: Rows<'_>, out: &mut Vec<u16>) {
    out.clear();
    out.reserve(rows.total());
    for row in rows.iter() {
        unpack_append(single, rows.pixels, row, out);
    }
}

/// The pixel count and the bytes it occupies, or why not.
fn geometry(
    format: PixelFormat,
    width: u32,
    height: u32,
    src: &[u8],
) -> Result<(usize, &[u8]), ConvertError> {
    let too_large = ConvertError::TooLarge { width, height };
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or(too_large.clone())?;
    // Every format with a layout declares a size; `None` here is overflow.
    let completed = format.image_bytes(width, height).ok_or(too_large.clone())?;
    let expected = match layout(format) {
        // A bit-packed run needs only its pixels' bits: a camera may stop
        // inside the last packing group (the module's *Geometry*).
        Some(Layout::Mono(single) | Layout::Bayer(single, _))
            if matches!(single.packing, Packing::Lsb | Packing::Gige) =>
        {
            pixels
                .checked_mul(usize::from(single.bits))
                .ok_or(too_large)?
                .div_ceil(8)
        }
        _ => completed,
    };
    if src.len() < expected {
        return Err(ConvertError::ShortPayload {
            format,
            expected,
            actual: src.len(),
        });
    }
    Ok((pixels, &src[..completed.min(src.len())]))
}

// ── Layouts ──────────────────────────────────────────────────────────────────

/// How a format's bytes become samples.
#[derive(Clone, Copy)]
enum Layout {
    Mono(Single),
    /// The mosaic's offsets to RGGB, as [`PixelFormat::cfa_pattern`] gives.
    Bayer(Single, (u8, u8)),
    Rgb(RgbOrder),
    Yuv(YuvOrder),
}

/// One component per pixel.
#[derive(Clone, Copy)]
struct Single {
    bits: u8,
    packing: Packing,
}

impl Single {
    fn is_8_bit(self) -> bool {
        self.bits == 8
    }
}

#[derive(Clone, Copy)]
enum Packing {
    /// PFNC lsb unpacked: one byte, or two little-endian bytes.
    Unpacked,
    /// PFNC lsb packed (`p`): an lsb-first bit stream.
    Lsb,
    /// GigE Vision 2.0 `10Packed`/`12Packed`: two pixels in three bytes, the
    /// eight msb of each in bytes 0 and 2, their lsb in the low and high
    /// nibble of byte 1.
    Gige,
}

#[derive(Clone, Copy)]
enum RgbOrder {
    Rgb,
    Bgr,
    Rgba,
    Bgra,
}

impl RgbOrder {
    fn stride(self) -> usize {
        match self {
            RgbOrder::Rgb | RgbOrder::Bgr => 3,
            RgbOrder::Rgba | RgbOrder::Bgra => 4,
        }
    }

    /// Where R, G and B sit in a pixel.
    fn indices(self) -> [usize; 3] {
        match self {
            RgbOrder::Rgb | RgbOrder::Rgba => [0, 1, 2],
            RgbOrder::Bgr | RgbOrder::Bgra => [2, 1, 0],
        }
    }
}

/// Byte order of a Y'CbCr group (U = Cb, V = Cr).
#[derive(Clone, Copy)]
enum YuvOrder {
    /// 4:2:2, Y0 U Y1 V — PFNC's default 4:2:2 order (§3.1.3).
    Yuyv,
    /// 4:2:2, U Y0 V Y1.
    Uyvy,
    /// 4:1:1, U Y0 Y1 V Y2 Y3.
    Uyyvyy,
    /// 4:4:4, U Y V.
    Uyv,
    /// 4:4:4, Y U V.
    Yuv,
}

fn layout(format: PixelFormat) -> Option<Layout> {
    use PixelFormat as P;
    let single = |bits, packing| Single { bits, packing };
    let unpacked = |bits| single(bits, Packing::Unpacked);
    let mono_or_bayer = |s: Single| match format.cfa_pattern() {
        Some((_, x, y)) => Layout::Bayer(s, (x, y)),
        None => Layout::Mono(s),
    };
    Some(match format {
        P::Mono8 | P::BayerRG8 | P::BayerGR8 | P::BayerGB8 | P::BayerBG8 => {
            mono_or_bayer(unpacked(8))
        }
        P::Mono10 | P::BayerRG10 | P::BayerGR10 | P::BayerGB10 | P::BayerBG10 => {
            mono_or_bayer(unpacked(10))
        }
        P::Mono12 | P::BayerRG12 | P::BayerGR12 | P::BayerGB12 | P::BayerBG12 => {
            mono_or_bayer(unpacked(12))
        }
        P::Mono14 => mono_or_bayer(unpacked(14)),
        P::Mono16 | P::BayerRG16 | P::BayerGR16 | P::BayerGB16 | P::BayerBG16 => {
            mono_or_bayer(unpacked(16))
        }
        P::Mono10p | P::BayerRG10p | P::BayerGR10p | P::BayerGB10p | P::BayerBG10p => {
            mono_or_bayer(single(10, Packing::Lsb))
        }
        P::Mono12p | P::BayerRG12p | P::BayerGR12p | P::BayerGB12p | P::BayerBG12p => {
            mono_or_bayer(single(12, Packing::Lsb))
        }
        P::Mono10Packed
        | P::BayerRG10Packed
        | P::BayerGR10Packed
        | P::BayerGB10Packed
        | P::BayerBG10Packed => mono_or_bayer(single(10, Packing::Gige)),
        P::Mono12Packed
        | P::BayerRG12Packed
        | P::BayerGR12Packed
        | P::BayerGB12Packed
        | P::BayerBG12Packed => mono_or_bayer(single(12, Packing::Gige)),
        P::RGB8Packed => Layout::Rgb(RgbOrder::Rgb),
        P::BGR8Packed => Layout::Rgb(RgbOrder::Bgr),
        P::RGBa8 => Layout::Rgb(RgbOrder::Rgba),
        P::BGRa8 => Layout::Rgb(RgbOrder::Bgra),
        P::YUV422_8 | P::YCbCr422_8 => Layout::Yuv(YuvOrder::Yuyv),
        P::YUV422_8_UYVY | P::YCbCr422_8_CbYCrY => Layout::Yuv(YuvOrder::Uyvy),
        P::YUV411_8_UYYVYY => Layout::Yuv(YuvOrder::Uyyvyy),
        P::YUV8_UYV | P::YCbCr8_CbYCr => Layout::Yuv(YuvOrder::Uyv),
        P::YCbCr8 => Layout::Yuv(YuvOrder::Yuv),
        _ => return None,
    })
}

// ── Unpacking and normalising ────────────────────────────────────────────────

/// Append `pixels` samples of `src`, which [`rows`] has sized.
fn unpack_append(single: Single, pixels: usize, src: &[u8], out: &mut Vec<u16>) {
    let start = out.len();
    out.reserve(pixels);
    match single.packing {
        Packing::Unpacked if single.is_8_bit() => {
            out.extend(src[..pixels].iter().map(|&v| u16::from(v)));
        }
        Packing::Unpacked => {
            out.extend(
                src.as_chunks::<2>().0[..pixels]
                    .iter()
                    .map(|pair| u16::from_le_bytes(*pair)),
            );
        }
        Packing::Lsb => {
            let bits = u32::from(single.bits);
            let mask = (1u32 << bits) - 1;
            let (mut acc, mut held) = (0u32, 0u32);
            let mut bytes = src.iter();
            for _ in 0..pixels {
                while held < bits {
                    acc |= u32::from(bytes.next().copied().unwrap_or(0)) << held;
                    held += 8;
                }
                out.push((acc & mask) as u16);
                acc >>= bits;
                held -= bits;
            }
        }
        Packing::Gige => {
            let shift = single.bits - 8;
            let mask = (1u16 << shift) - 1;
            let (groups, tail) = src.as_chunks::<3>();
            for &[msb0, lsb, msb1] in groups {
                out.push((u16::from(msb0) << shift) | (u16::from(lsb) & mask));
                out.push((u16::from(msb1) << shift) | ((u16::from(lsb) >> 4) & mask));
            }
            // A run that stops inside its last group: that group's first
            // pixel, from its eight msb and the low nibble after them.
            if let &[msb0, lsb, ..] = tail {
                out.push((u16::from(msb0) << shift) | (u16::from(lsb) & mask));
            }
            // A completed last group carries one pixel too many.
            out.truncate(start + pixels);
        }
    }
}

/// A sample's top eight bits.
fn narrow(value: u16, bits: u8) -> u8 {
    // A camera that sets padding bits would overflow; saturate rather than wrap.
    (value >> (bits - 8)).min(0xFF) as u8
}

fn narrow_into(samples: &[u16], bits: u8, out: &mut Vec<u8>) {
    out.clear();
    out.extend(samples.iter().map(|&v| narrow(v, bits)));
}

// ── Demosaicing ──────────────────────────────────────────────────────────────

/// A sample type the demosaic averages in `u32`.
trait Sample: Copy + Default {
    fn widen(self) -> u32;
    /// `value` is a mean of samples, so it fits.
    fn narrow(value: u32) -> Self;
}

impl Sample for u8 {
    fn widen(self) -> u32 {
        u32::from(self)
    }
    fn narrow(value: u32) -> Self {
        value as u8
    }
}

impl Sample for u16 {
    fn widen(self) -> u32 {
        u32::from(self)
    }
    fn narrow(value: u32) -> Self {
        value as u16
    }
}

/// Bilinear demosaic of a `width × height` mosaic whose pixel `(x, y)` has
/// the colour RGGB has at `(x + offsets.0, y + offsets.1)`. See the module
/// documentation for the edge rule.
fn demosaic_bilinear<T: Sample>(
    mosaic: &[T],
    width: usize,
    height: usize,
    (x_offset, y_offset): (u8, u8),
    out: &mut Vec<T>,
) {
    out.clear();
    out.resize(width * height * 3, T::default());
    if width == 0 || height == 0 {
        return;
    }
    let row = |y: usize| &mosaic[y * width..][..width];
    for y in 0..height {
        let (above, here, below) = (
            row(y.saturating_sub(1)),
            row(y),
            row((y + 1).min(height - 1)),
        );
        let y_site = (y + usize::from(y_offset)) & 1;
        for x in 0..width {
            let (left, right) = (x.saturating_sub(1), (x + 1).min(width - 1));
            let at = |line: &[T], i: usize| line[i].widen();
            let centre = at(here, x);
            let horizontal = || (at(here, left) + at(here, right)) / 2;
            let vertical = || (at(above, x) + at(below, x)) / 2;
            let cross = || (at(here, left) + at(here, right) + at(above, x) + at(below, x)) / 4;
            let diagonal =
                || (at(above, left) + at(above, right) + at(below, left) + at(below, right)) / 4;
            let (r, g, b) = match ((x + usize::from(x_offset)) & 1, y_site) {
                // Red site.
                (0, 0) => (centre, cross(), diagonal()),
                // Green between reds (left/right) and blues (above/below).
                (1, 0) => (horizontal(), centre, vertical()),
                // Green between blues (left/right) and reds (above/below).
                (0, 1) => (vertical(), centre, horizontal()),
                // Blue site.
                _ => (diagonal(), cross(), centre),
            };
            let px = &mut out[(y * width + x) * 3..][..3];
            px.copy_from_slice(&[T::narrow(r), T::narrow(g), T::narrow(b)]);
        }
    }
}

// ── Y'CbCr ───────────────────────────────────────────────────────────────────

/// PFNC 2.4 §8.2.1, generic full-scale Y'CbCr to R'G'B' (8-bit):
/// R' = Y' + 1.402 (Cr − 128), G' = Y' − 0.34414 (Cb − 128) − 0.71414 (Cr − 128),
/// B' = Y' + 1.772 (Cb − 128), rounded and clamped to 0-255.
fn ycbcr_to_rgb(y: u8, cb: u8, cr: u8) -> [u8; 3] {
    let y = f32::from(y);
    let cb = f32::from(cb) - 128.0;
    let cr = f32::from(cr) - 128.0;
    let channel = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    [
        channel(y + 1.402 * cr),
        channel(y - 0.34414 * cb - 0.71414 * cr),
        channel(y + 1.772 * cb),
    ]
}

/// Append `pixels` pixels of `src` as RGB.
fn yuv_append(order: YuvOrder, pixels: usize, src: &[u8], out: &mut Vec<u8>) {
    let start = out.len();
    // (bytes per group, the luma offsets, the Cb offset, the Cr offset)
    let (group, lumas, cb, cr): (usize, &[usize], usize, usize) = match order {
        YuvOrder::Yuyv => (4, &[0, 2], 1, 3),
        YuvOrder::Uyvy => (4, &[1, 3], 0, 2),
        YuvOrder::Uyyvyy => (6, &[1, 2, 4, 5], 0, 3),
        YuvOrder::Uyv => (3, &[1], 0, 2),
        YuvOrder::Yuv => (3, &[0], 1, 2),
    };
    out.reserve(pixels * 3);
    for chunk in src.chunks_exact(group) {
        for &luma in lumas {
            out.extend_from_slice(&ycbcr_to_rgb(chunk[luma], chunk[cb], chunk[cr]));
        }
    }
    // A completed last group carries pixels past the image.
    out.truncate(start + pixels * 3);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PixelFormat as P;

    fn unpack(format: P, width: u32, height: u32, src: &[u8]) -> Vec<u16> {
        unpack_u16(format, width, height, None, src).unwrap()
    }

    // ── Unpacking: golden vectors from the standards ─────────────────────────

    /// PFNC 2.4 §6.1.1, Figures 6-2 and 6-3: lsb unpacked, the sample in the
    /// low bits of two little-endian bytes.
    #[test]
    fn mono10_and_mono12_are_lsb_aligned_in_two_bytes() {
        assert_eq!(unpack(P::Mono10, 1, 1, &[0xA5, 0x02]), [0x2A5]);
        assert_eq!(
            unpack(P::Mono12, 2, 1, &[0xBC, 0x0A, 0x23, 0x01]),
            [0xABC, 0x123]
        );
        assert_eq!(unpack(P::Mono16, 1, 1, &[0x34, 0x12]), [0x1234]);
        assert_eq!(unpack(P::Mono8, 2, 1, &[7, 250]), [7, 250]);
    }

    /// PFNC 2.4 §6.3.1, Figure 6-9 (Mono10p): byte 0 holds L1 bits 7-0;
    /// byte 1 L2 bits 5-0 over L1 bits 9-8; byte 2 L3 bits 3-0 over L2 bits
    /// 9-6; byte 3 L4 bits 1-0 over L3 bits 9-4; byte 4 L4 bits 9-2.
    #[test]
    fn mono10p_follows_figure_6_9() {
        // L1 = 0x2A5, L2 = 0x15A, L3 = 0x0F0, L4 = 0x30F.
        let bytes = [0xA5, 0x6A, 0x05, 0xCF, 0xC3];
        assert_eq!(
            unpack(P::Mono10p, 4, 1, &bytes),
            [0x2A5, 0x15A, 0x0F0, 0x30F]
        );
        assert_eq!(
            unpack(P::BayerRG10p, 2, 2, &bytes),
            [0x2A5, 0x15A, 0x0F0, 0x30F]
        );
    }

    /// PFNC 2.4 §6.3.1, "lsb Packed" construction rules: the first
    /// component's bit 0 is bit 0 of byte 0, the next component is appended
    /// from its own lsb. For 12 bits: byte 0 = L1 bits 7-0, byte 1 = L2 bits
    /// 3-0 over L1 bits 11-8, byte 2 = L2 bits 11-4.
    #[test]
    fn mono12p_is_lsb_packed() {
        let bytes = [0xBC, 0x3A, 0x12];
        assert_eq!(unpack(P::Mono12p, 2, 1, &bytes), [0xABC, 0x123]);
        assert_eq!(unpack(P::BayerGB12p, 2, 1, &bytes), [0xABC, 0x123]);
    }

    /// GigE Vision 2.0 Mono12Packed, as Teledyne FLIR documents it (the GigE
    /// Vision text is not public; the EMVA values table assigns the code to
    /// it): "The first byte … contains the eight msb of the first pixel. The
    /// third byte contains the eight msb of the second pixel. The four lsb of
    /// the second byte contains four lsb of the first pixel, and the rest of
    /// the second byte is packed with the four lsb of the second pixel."
    #[test]
    fn mono12packed_is_the_gige_vision_layout() {
        let bytes = [0xAB, 0x3C, 0x12];
        assert_eq!(unpack(P::Mono12Packed, 2, 1, &bytes), [0xABC, 0x123]);
        // The same source says the Bayer*12Packed formats share the layout.
        assert_eq!(unpack(P::BayerBG12Packed, 2, 1, &bytes), [0xABC, 0x123]);
    }

    /// GigE Vision 2.0 Mono10Packed, same source: bytes 0 and 2 hold the eight
    /// msb of the two pixels; bits 1-0 of byte 1 the first pixel's two lsb,
    /// bits 5-4 the second's; the remaining bits are zero.
    #[test]
    fn mono10packed_is_the_gige_vision_layout() {
        let bytes = [0xA9, 0x21, 0x56];
        assert_eq!(unpack(P::Mono10Packed, 2, 1, &bytes), [0x2A5, 0x15A]);
        assert_eq!(unpack(P::BayerGR10Packed, 2, 1, &bytes), [0x2A5, 0x15A]);
    }

    /// The reason a round trip proves nothing: the two 12-bit packings read
    /// the same three bytes as different pixels.
    #[test]
    fn the_two_12_bit_packings_disagree_on_the_same_bytes() {
        let bytes = [0xAB, 0x3C, 0x12];
        assert_eq!(unpack(P::Mono12p, 2, 1, &bytes), [0xCAB, 0x123]);
        assert_ne!(
            unpack(P::Mono12p, 2, 1, &bytes),
            unpack(P::Mono12Packed, 2, 1, &bytes)
        );
    }

    /// An image that ends inside a packing group needs only the bytes its
    /// pixels' bits fill. PFNC 2.4 §6.7 completes the group with zero
    /// components, but an Allied Vision Manta G-125B at 1291 × 963 in
    /// `Mono12Packed` — 1 243 233 pixels, an odd count — gave `PayloadSize`
    /// 1 864 850 and sent frames of that length: ⌈1 243 233 × 12 / 8⌉, one
    /// byte short of the completed group. A completed group still decodes.
    #[test]
    fn a_partial_last_group_needs_only_its_pixels_bits() {
        // The last pixel's eight msb, then its four lsb in the next byte.
        let manta = [0xAB, 0x3C, 0x12, 0x45, 0x06];
        assert_eq!(unpack(P::Mono12Packed, 3, 1, &manta), [0xABC, 0x123, 0x456]);
        assert_eq!(
            unpack(P::Mono12Packed, 3, 1, &[0xAB, 0x3C, 0x12, 0x45, 0x06, 0x00]),
            [0xABC, 0x123, 0x456],
            "the group completed"
        );
        assert_eq!(unpack(P::Mono12p, 1, 1, &[0xBC, 0x0A]), [0xABC]);
        assert_eq!(unpack(P::Mono12p, 1, 1, &[0xBC, 0x0A, 0x00]), [0xABC]);
        assert_eq!(
            unpack_u16(P::Mono12Packed, 3, 1, None, &manta[..4]),
            Err(ConvertError::ShortPayload {
                format: P::Mono12Packed,
                expected: 5,
                actual: 4
            })
        );
        // The Manta's own geometry, through every conversion.
        let (width, height) = (1291, 963);
        let src = vec![0x80; 1_864_850];
        let mut conv = Converter::new();
        assert_eq!(
            conv.gray8(P::Mono12Packed, width, height, None, &src)
                .map(<[u8]>::len),
            Ok(1_243_233)
        );
        assert!(to_rgb8(P::Mono12Packed, width, height, None, &src).is_ok());
    }

    /// A GVSP image leader declares `padding_x` bytes after every line
    /// (Codex review of #196): with a row stride each line starts where it
    /// says, not where the previous one's pixels ended.
    #[test]
    fn padded_lines_are_read_from_their_own_start() {
        // Mono8, 3 × 2, two bytes of padding after each line.
        let src = [1, 2, 3, 0xEE, 0xEE, 4, 5, 6, 0xEE, 0xEE];
        assert_eq!(
            unpack_u16(P::Mono8, 3, 2, Some(5), &src).unwrap(),
            [1, 2, 3, 4, 5, 6]
        );
        let mut conv = Converter::new();
        assert_eq!(
            conv.gray8(P::Mono8, 3, 2, Some(5), &src).unwrap(),
            [1, 2, 3, 4, 5, 6]
        );
        // The last line needs no padding after it.
        assert!(unpack_u16(P::Mono8, 3, 2, Some(5), &src[..8]).is_ok());
        // RGB8, 1 × 2, one byte of padding.
        let rgb = [1, 2, 3, 0xEE, 4, 5, 6, 0xEE];
        assert_eq!(
            to_rgb8(P::RGB8Packed, 1, 2, Some(4), &rgb).unwrap(),
            [1, 2, 3, 4, 5, 6]
        );
        assert_eq!(
            conv.rgb8(P::RGB8Packed, 1, 2, Some(4), &rgb).unwrap(),
            [1, 2, 3, 4, 5, 6]
        );
    }

    /// A padded `Mono12p` line of three pixels ends inside its second
    /// packing group, which the line completes (PFNC 2.4 §6.7): six bytes,
    /// then the padding, then the next line.
    #[test]
    fn a_padded_packed_line_completes_its_own_group() {
        let src = [
            0x23, 0x61, 0x45, 0x89, 0x07, 0x00, 0xEE, // 0x123 0x456 0x789 (+0) | pad
            0x11, 0x21, 0x22, 0x33, 0x03, 0x00, 0xEE, // 0x111 0x222 0x333 (+0) | pad
        ];
        assert_eq!(
            unpack_u16(P::Mono12p, 3, 2, Some(7), &src).unwrap(),
            [0x123, 0x456, 0x789, 0x111, 0x222, 0x333]
        );
        assert_eq!(
            unpack_u16(P::Mono12p, 3, 2, Some(5), &src),
            Err(ConvertError::StrideTooShort { stride: 5, line: 6 })
        );
    }

    #[test]
    fn short_payloads_are_refused_and_trailing_bytes_ignored() {
        assert_eq!(
            to_rgb8(P::Mono8, 2, 2, None, &[1, 2, 3]),
            Err(ConvertError::ShortPayload {
                format: P::Mono8,
                expected: 4,
                actual: 3
            })
        );
        assert_eq!(
            to_rgb8(P::Mono8, 1, 1, None, &[9, 0xEE, 0xEE]).unwrap(),
            [9, 9, 9]
        );
    }

    /// `supports` answers exactly for the formats `to_rgb8` converts.
    #[test]
    fn supports_is_true_exactly_for_the_formats_converted() {
        let mut supported = 0;
        for &format in P::ALL {
            let Some(len) = format.image_bytes(4, 2) else {
                assert!(!supports(format), "{format}");
                continue;
            };
            let converts = to_rgb8(format, 4, 2, None, &vec![0; len]).is_ok();
            assert_eq!(supports(format), converts, "{format}");
            supported += usize::from(converts);
        }
        // 9 Mono, 32 Bayer, 4 RGB, 8 Y'CbCr.
        assert_eq!(supported, 53);
    }

    #[test]
    fn formats_without_a_conversion_say_so() {
        for format in [P::Coord3DC32f, P::Confidence8, P::Unknown(0x0123_4567)] {
            assert_eq!(
                to_rgb8(format, 1, 1, None, &[0; 16]),
                Err(ConvertError::Unsupported(format))
            );
        }
        let mut conv = Converter::new();
        assert_eq!(
            conv.gray8(P::RGB8Packed, 1, 1, None, &[1, 2, 3]),
            Err(ConvertError::Unsupported(P::RGB8Packed))
        );
    }

    // ── Normalising to 8 bits ────────────────────────────────────────────────

    /// The top eight bits of the camera's depth.
    #[test]
    fn deep_samples_keep_their_top_eight_bits() {
        let mut conv = Converter::new();
        assert_eq!(
            conv.gray8(P::Mono10, 1, 1, None, &[0xFF, 0x03]).unwrap(),
            [0xFF]
        );
        assert_eq!(
            conv.gray8(P::Mono12, 1, 1, None, &[0xF0, 0x0F]).unwrap(),
            [0xFF]
        );
        assert_eq!(
            conv.gray8(P::Mono14, 1, 1, None, &[0xFF, 0x3F]).unwrap(),
            [0xFF]
        );
        assert_eq!(
            conv.gray8(P::Mono16, 1, 1, None, &[0x00, 0x80]).unwrap(),
            [0x80]
        );
        // 0xABC >> 4 and 0x123 >> 4.
        assert_eq!(
            conv.gray8(P::Mono12p, 2, 1, None, &[0xBC, 0x3A, 0x12])
                .unwrap(),
            [0xAB, 0x12]
        );
        assert_eq!(
            conv.gray8(P::Mono12Packed, 2, 1, None, &[0xAB, 0x3C, 0x12])
                .unwrap(),
            [0xAB, 0x12]
        );
        assert_eq!(conv.gray8(P::Mono8, 2, 1, None, &[3, 4]).unwrap(), [3, 4]);
        assert_eq!(
            to_rgb8(P::Mono10p, 4, 1, None, &[0xA5, 0x6A, 0x05, 0xCF, 0xC3]).unwrap(),
            [
                0xA9, 0xA9, 0xA9, 0x56, 0x56, 0x56, 0x3C, 0x3C, 0x3C, 0xC3, 0xC3, 0xC3
            ]
        );
    }

    // ── Demosaicing ──────────────────────────────────────────────────────────

    /// A 4×4 mosaic with each site's colour written per PFNC 2.4 §3.1.7
    /// (R = 200, G = 100, B = 50 after normalising). Away from the border
    /// every neighbour is in the image, so bilinear interpolation of a flat
    /// colour must give it back exactly — for every pattern and depth.
    #[test]
    fn every_bayer_pattern_puts_colours_where_pfnc_3_1_7_does() {
        // Top-left 2×2 tile per Figures 3-7, 3-9, 3-11, 3-13.
        let tiles: [(&[P], [[char; 2]; 2]); 4] = [
            (
                &[P::BayerRG8, P::BayerRG10, P::BayerRG12, P::BayerRG16],
                [['R', 'G'], ['G', 'B']],
            ),
            (
                &[P::BayerGR8, P::BayerGR10, P::BayerGR12, P::BayerGR16],
                [['G', 'R'], ['B', 'G']],
            ),
            (
                &[P::BayerGB8, P::BayerGB10, P::BayerGB12, P::BayerGB16],
                [['G', 'B'], ['R', 'G']],
            ),
            (
                &[P::BayerBG8, P::BayerBG10, P::BayerBG12, P::BayerBG16],
                [['B', 'G'], ['G', 'R']],
            ),
        ];
        for (formats, tile) in tiles {
            for &format in formats {
                let bits = format.bits_per_pixel().unwrap();
                let depth = match format.to_string().trim_start_matches("Bayer")[2..].parse::<u32>()
                {
                    Ok(d) => d,
                    Err(_) => unreachable!(),
                };
                let mut src = Vec::new();
                for y in 0..4 {
                    for x in 0..4 {
                        let v8: u32 = match tile[y % 2][x % 2] {
                            'R' => 200,
                            'G' => 100,
                            _ => 50,
                        };
                        let v = v8 << (depth - 8);
                        if bits == 8 {
                            src.push(v as u8);
                        } else {
                            src.extend_from_slice(&(v as u16).to_le_bytes());
                        }
                    }
                }
                let rgb = to_rgb8(format, 4, 4, None, &src).unwrap();
                for (x, y) in [(1, 1), (2, 1), (1, 2), (2, 2)] {
                    let i = (y * 4 + x) * 3;
                    assert_eq!(&rgb[i..i + 3], [200, 100, 50], "{format} at ({x}, {y})");
                }
            }
        }
    }

    /// The arithmetic, by hand: bilinear interpolation with truncating
    /// integer means, edge samples replicated (clamp-to-edge) — the kernel
    /// Viva Studio's viewer used before this module existed.
    #[test]
    fn bilinear_demosaic_by_hand() {
        #[rustfmt::skip]
        let mosaic = [
             12, 200,  34, 180,
             90,   7, 150,   3,
             56, 210,  78, 190,
            120,   9, 140,  11,
        ];
        let rgb = to_rgb8(P::BayerRG8, 4, 4, None, &mosaic).unwrap();
        let at = |x: usize, y: usize| <[u8; 3]>::try_from(&rgb[(y * 4 + x) * 3..][..3]).unwrap();
        // B site: G = (90+150+200+210)/4 = 162, R = (12+34+56+78)/4 = 45.
        assert_eq!(at(1, 1), [45, 162, 7]);
        // G on a B row: R from above/below, B from left/right.
        assert_eq!(at(2, 1), [(34 + 78) / 2, 150, (7 + 3) / 2]);
        // G on an R row: R from left/right, B from above/below.
        assert_eq!(at(1, 2), [(56 + 78) / 2, 210, (7 + 9) / 2]);
        // R site: G = (210+190+150+140)/4 = 172, B = (7+3+9+11)/4 = 7.
        assert_eq!(at(2, 2), [78, 172, 7]);
        // Corner: the missing neighbours repeat the edge, R included.
        // G = (12+200+12+90)/4 = 78, B = (12+200+90+7)/4 = 77.
        assert_eq!(at(0, 0), [12, 78, 77]);
    }

    /// GigE Vision 12Packed Bayer, packed by hand: R = 0xC8F, G = 0x64A,
    /// B = 0x323, i.e. 200, 100 and 50 in their top eight bits. Read with the
    /// wrong 12-bit packing, R would come out as 0xFC8 >> 4 = 252.
    #[test]
    fn packed_bayer_is_unpacked_before_demosaicing() {
        let rg = [0xC8, 0xAF, 0x64];
        let gb = [0x64, 0x3A, 0x32];
        let mut src = Vec::new();
        for row in [rg, gb, rg, gb] {
            src.extend_from_slice(&row);
            src.extend_from_slice(&row);
        }
        let rgb = to_rgb8(P::BayerRG12Packed, 4, 4, None, &src).unwrap();
        assert_eq!(&rgb[(4 + 1) * 3..][..3], [200, 100, 50]);
        assert_eq!(&rgb[(2 * 4 + 2) * 3..][..3], [200, 100, 50]);

        let Pixels::Rgb16 { samples, bits } =
            full_depth(P::BayerRG12Packed, 4, 4, None, &src).unwrap()
        else {
            panic!("a 12-bit mosaic keeps its depth");
        };
        assert_eq!(bits, 12);
        assert_eq!(&samples[(4 + 1) * 3..][..3], [0xC8F, 0x64A, 0x323]);
    }

    // ── Y'CbCr ───────────────────────────────────────────────────────────────

    /// PFNC 2.4 §8.2.1 reverse equations, worked by hand for Y' = 100,
    /// Cb = 150, Cr = 90:
    ///   R' = 100 + 1.402 × (−38)                  =  46.72 → 47
    ///   G' = 100 − 0.34414 × 22 − 0.71414 × (−38) = 119.57 → 120
    ///   B' = 100 + 1.772 × 22                     = 138.98 → 139
    /// and for Y' = 200 with the same chroma: 147, 220, 239.
    const PX0: [u8; 3] = [47, 120, 139];
    const PX1: [u8; 3] = [147, 220, 239];

    #[test]
    fn yuv422_component_orders() {
        // §3.1.3: the default 4:2:2 order is Y U Y V; `_UYVY` and `_CbYCrY`
        // name theirs (§7.3).
        for (format, bytes) in [
            (P::YUV422_8, [100, 150, 200, 90]),
            (P::YCbCr422_8, [100, 150, 200, 90]),
            (P::YUV422_8_UYVY, [150, 100, 90, 200]),
            (P::YCbCr422_8_CbYCrY, [150, 100, 90, 200]),
        ] {
            assert_eq!(
                to_rgb8(format, 2, 1, None, &bytes).unwrap(),
                [PX0, PX1].concat(),
                "{format}"
            );
        }
    }

    #[test]
    fn yuv411_and_444_component_orders() {
        assert_eq!(
            to_rgb8(
                P::YUV411_8_UYYVYY,
                4,
                1,
                None,
                &[150, 100, 200, 90, 100, 200]
            )
            .unwrap(),
            [PX0, PX1, PX0, PX1].concat()
        );
        assert_eq!(
            to_rgb8(P::YUV8_UYV, 1, 1, None, &[150, 100, 90]).unwrap(),
            PX0
        );
        assert_eq!(
            to_rgb8(P::YCbCr8_CbYCr, 1, 1, None, &[150, 100, 90]).unwrap(),
            PX0
        );
        assert_eq!(
            to_rgb8(P::YCbCr8, 1, 1, None, &[100, 150, 90]).unwrap(),
            PX0
        );
    }

    /// §8.2.1: "Values must be truncated to fit in that range."
    /// Y' = 250, Cb = 128, Cr = 255: R' = 250 + 1.402 × 127 = 428 → 255,
    /// G' = 250 − 0.71414 × 127 = 159.3 → 159, B' = 250.
    #[test]
    fn ycbcr_results_are_clamped_to_the_8_bit_range() {
        assert_eq!(
            to_rgb8(P::YCbCr8, 1, 1, None, &[250, 128, 255]).unwrap(),
            [255, 159, 250]
        );
    }

    /// §6.7: an odd last pixel still arrives in a whole 4:2:2 group.
    #[test]
    fn an_odd_yuv422_width_reads_the_completed_group() {
        let rgb = to_rgb8(
            P::YUV422_8,
            3,
            1,
            None,
            &[100, 150, 200, 90, 100, 150, 0, 90],
        )
        .unwrap();
        assert_eq!(rgb, [PX0, PX1, PX0].concat());
    }

    // ── RGB family ───────────────────────────────────────────────────────────

    #[test]
    fn rgb_orders_and_alpha() {
        assert_eq!(
            to_rgb8(P::RGB8Packed, 1, 1, None, &[1, 2, 3]).unwrap(),
            [1, 2, 3]
        );
        assert_eq!(
            to_rgb8(P::BGR8Packed, 1, 1, None, &[1, 2, 3]).unwrap(),
            [3, 2, 1]
        );
        assert_eq!(
            to_rgb8(P::RGBa8, 1, 1, None, &[1, 2, 3, 99]).unwrap(),
            [1, 2, 3]
        );
        assert_eq!(
            to_rgb8(P::BGRa8, 1, 1, None, &[1, 2, 3, 99]).unwrap(),
            [3, 2, 1]
        );
        let mut conv = Converter::new();
        let src = [4, 5, 6];
        let out = conv.rgb8(P::RGB8Packed, 1, 1, None, &src).unwrap();
        assert!(core::ptr::eq(out, &src[..]), "RGB8 is lent, not copied");
    }

    // ── Full depth ───────────────────────────────────────────────────────────

    #[test]
    fn full_depth_keeps_the_camera_values() {
        assert_eq!(
            full_depth(P::Mono12p, 2, 1, None, &[0xBC, 0x3A, 0x12]).unwrap(),
            Pixels::Gray16 {
                samples: vec![0xABC, 0x123],
                bits: 12
            }
        );
        assert_eq!(
            full_depth(P::Mono8, 2, 1, None, &[1, 2]).unwrap(),
            Pixels::Gray8(vec![1, 2])
        );
        assert_eq!(
            full_depth(P::BGR8Packed, 1, 1, None, &[1, 2, 3]).unwrap(),
            Pixels::Rgb8(vec![3, 2, 1])
        );
        assert_eq!(
            full_depth(P::Mono10, 1, 1, None, &[0xA5, 0x02])
                .unwrap()
                .into_rgb(),
            Pixels::Rgb16 {
                samples: vec![0x2A5; 3],
                bits: 10
            }
        );
        assert_eq!(
            full_depth(P::Mono8, 1, 1, None, &[7]).unwrap().into_rgb(),
            Pixels::Rgb8(vec![7; 3])
        );
    }

    /// The converter's buffers are reused: a second frame of the same size
    /// does not allocate.
    #[test]
    fn the_converter_reuses_its_buffers() {
        let mut conv = Converter::new();
        let src = [0u8; 16 * 2];
        let first = conv.rgb8(P::BayerRG12, 4, 4, None, &src).unwrap().as_ptr();
        let second = conv.rgb8(P::BayerRG12, 4, 4, None, &src).unwrap().as_ptr();
        assert_eq!(first, second);
    }
}
