//! Saving frames as PNG or TIFF at the camera's bit depth.
//!
//! Behind the `image-io` feature. A frame is first converted by
//! [`viva_pfnc::convert::full_depth`]: monochrome stays grey, everything else
//! becomes RGB, and samples wider than 8 bits — Mono10/12/14/16 packed or
//! not, and Bayer mosaics demosaiced at their own depth — are written as
//! 16-bit images.
//!
//! **16-bit samples are scaled to the full 16-bit range**, as PNG requires
//! of an encoder whose samples are narrower than the file's (PNG 3rd edition
//! §12.4), by left bit replication: a 12-bit `0xABC` is written `0xABCA`.
//! `value >> (16 − bits)` gives the camera's value back exactly. A PNG
//! records `bits` in its `sBIT` chunk (§11.3.2.4); TIFF has no such field,
//! so a TIFF holds the same scaled samples and the depth is yours to know.
//! Samples of 8 or 16 bits are written unchanged.
//!
//! Files are uncompressed TIFF and default-compression PNG.

use std::io::Cursor;
use std::path::Path;

use viva_pfnc::convert::{self, ConvertError, Pixels};

use crate::Frame;

/// An image file format [`encode`] writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImageFormat {
    /// PNG: 8- or 16-bit grey or RGB, the camera's depth in `sBIT`.
    Png,
    /// Baseline TIFF, uncompressed: 8- or 16-bit grey or RGB.
    Tiff,
}

impl ImageFormat {
    /// The format a file name asks for: `.png`, or `.tif`/`.tiff`, in any
    /// case.
    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "png" => Some(ImageFormat::Png),
            "tif" | "tiff" => Some(ImageFormat::Tiff),
            _ => None,
        }
    }

    /// The usual file extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Tiff => "tiff",
        }
    }
}

/// Why an image could not be written.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ImageIoError {
    /// The frame's pixels could not be converted.
    #[error(transparent)]
    Convert(#[from] ConvertError),
    /// `pixels` is not `width × height` samples, or the encoder refused it.
    #[error("cannot encode image: {0}")]
    Encode(String),
    /// Writing the file failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A frame as an image file, at its own depth (see the [module](self)
/// documentation).
pub fn encode_frame(frame: &Frame, format: ImageFormat) -> Result<Vec<u8>, ImageIoError> {
    let pixels = convert::full_depth(
        frame.pixel_format,
        frame.width,
        frame.height,
        &frame.payload,
    )?;
    encode(&pixels, frame.width, frame.height, format)
}

/// Write a frame to `path`, in the format its extension names.
pub fn save_frame(frame: &Frame, path: &Path) -> Result<(), ImageIoError> {
    let format = ImageFormat::from_path(path).ok_or_else(|| {
        ImageIoError::Encode(format!(
            "{} does not end in .png, .tif or .tiff",
            path.display()
        ))
    })?;
    std::fs::write(path, encode_frame(frame, format)?)?;
    Ok(())
}

/// A `width × height` image as a file; [`Pixels::into_rgb`] first for colour.
pub fn encode(
    pixels: &Pixels,
    width: u32,
    height: u32,
    format: ImageFormat,
) -> Result<Vec<u8>, ImageIoError> {
    let image = Image::of(pixels, width, height)?;
    match format {
        ImageFormat::Png => image.png(),
        ImageFormat::Tiff => image.tiff(),
    }
}

/// Samples ready for a file: 16-bit ones already scaled.
struct Image<'a> {
    width: u32,
    height: u32,
    rgb: bool,
    samples: Samples<'a>,
    /// The camera's depth when it is narrower than the file's 16 bits.
    significant: Option<u8>,
}

enum Samples<'a> {
    Eight(&'a [u8]),
    Sixteen(Vec<u16>),
}

impl<'a> Image<'a> {
    fn of(pixels: &'a Pixels, width: u32, height: u32) -> Result<Self, ImageIoError> {
        let (rgb, samples, bits) = match pixels {
            Pixels::Gray8(v) => (false, Samples::Eight(v), 8),
            Pixels::Rgb8(v) => (true, Samples::Eight(v), 8),
            Pixels::Gray16 { samples, bits } => {
                (false, Samples::Sixteen(scale(samples, *bits)), *bits)
            }
            Pixels::Rgb16 { samples, bits } => {
                (true, Samples::Sixteen(scale(samples, *bits)), *bits)
            }
            _ => return Err(ImageIoError::Encode("unknown pixel layout".into())),
        };
        let channels = if rgb { 3 } else { 1 };
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(channels));
        let actual = match &samples {
            Samples::Eight(v) => v.len(),
            Samples::Sixteen(v) => v.len(),
        };
        if expected != Some(actual) {
            return Err(ImageIoError::Encode(format!(
                "{actual} samples for a {width}×{height} image with {channels} channel(s)"
            )));
        }
        Ok(Image {
            width,
            height,
            rgb,
            samples,
            significant: (8 < bits && bits < 16).then_some(bits),
        })
    }

    fn png(&self) -> Result<Vec<u8>, ImageIoError> {
        let failed = |e: png::EncodingError| ImageIoError::Encode(e.to_string());
        let mut file = Vec::new();
        let mut encoder = png::Encoder::new(&mut file, self.width, self.height);
        encoder.set_color(if self.rgb {
            png::ColorType::Rgb
        } else {
            png::ColorType::Grayscale
        });
        let data: Vec<u8> = match &self.samples {
            Samples::Eight(v) => {
                encoder.set_depth(png::BitDepth::Eight);
                v.to_vec()
            }
            Samples::Sixteen(v) => {
                encoder.set_depth(png::BitDepth::Sixteen);
                // §7.2: network byte order.
                v.iter().flat_map(|s| s.to_be_bytes()).collect()
            }
        };
        let mut writer = encoder.write_header().map_err(failed)?;
        if let Some(bits) = self.significant {
            // §11.3.2.4: one byte per channel, before IDAT.
            let channels = if self.rgb { 3 } else { 1 };
            writer
                .write_chunk(png::chunk::sBIT, &[bits; 3][..channels])
                .map_err(failed)?;
        }
        writer.write_image_data(&data).map_err(failed)?;
        writer.finish().map_err(failed)?;
        Ok(file)
    }

    fn tiff(&self) -> Result<Vec<u8>, ImageIoError> {
        use tiff::encoder::{TiffEncoder, colortype};
        let failed = |e: tiff::TiffError| ImageIoError::Encode(e.to_string());
        let mut file = Cursor::new(Vec::new());
        let mut encoder = TiffEncoder::new(&mut file).map_err(failed)?;
        let (w, h) = (self.width, self.height);
        match (&self.samples, self.rgb) {
            (Samples::Eight(v), false) => encoder.write_image::<colortype::Gray8>(w, h, v),
            (Samples::Eight(v), true) => encoder.write_image::<colortype::RGB8>(w, h, v),
            (Samples::Sixteen(v), false) => encoder.write_image::<colortype::Gray16>(w, h, v),
            (Samples::Sixteen(v), true) => encoder.write_image::<colortype::RGB16>(w, h, v),
        }
        .map_err(failed)?;
        Ok(file.into_inner())
    }
}

/// PNG 3rd edition §12.4, left bit replication: the `bits` valid bits moved
/// to the top of 16, their own top bits repeated into the bits left open.
fn scale(samples: &[u16], bits: u8) -> Vec<u16> {
    if bits >= 16 {
        return samples.to_vec();
    }
    let open = 16 - u32::from(bits);
    samples
        .iter()
        .map(|&v| (v << open) | (v >> (u32::from(bits) - open)))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use bytes::Bytes;
    use viva_pfnc::PixelFormat;

    use super::*;

    fn frame(format: PixelFormat, width: u32, height: u32, payload: &[u8]) -> Frame {
        Frame::new(Bytes::copy_from_slice(payload), width, height, format)
    }

    /// (colour type, bit depth, sBIT, samples as big-endian bytes).
    fn read_png(file: &[u8]) -> (png::ColorType, png::BitDepth, Option<Vec<u8>>, Vec<u8>) {
        let mut reader = png::Decoder::new(Cursor::new(file)).read_info().unwrap();
        let info = reader.info();
        let (colour, depth) = (info.color_type, info.bit_depth);
        let sbit = info.sbit.as_ref().map(|s| s.to_vec());
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let out = reader.next_frame(&mut buf).unwrap();
        buf.truncate(out.buffer_size());
        (colour, depth, sbit, buf)
    }

    /// PNG 3rd edition §12.4: a 12-bit sample is scaled up to 16 bits by left
    /// bit replication — 0xABC becomes 0xABC0 | 0xA, 0x123 becomes 0x1231 —
    /// and §11.3.2.4 `sBIT` records the 12 significant bits, so `>> 4` gives
    /// the camera's values back. §7.2: 16-bit samples are big-endian.
    #[test]
    fn mono12_is_a_16_bit_grey_png_with_its_depth_in_sbit() {
        let file = encode_frame(
            &frame(PixelFormat::Mono12p, 2, 1, &[0xBC, 0x3A, 0x12]),
            ImageFormat::Png,
        )
        .unwrap();
        let (colour, depth, sbit, samples) = read_png(&file);
        assert_eq!(
            (colour, depth),
            (png::ColorType::Grayscale, png::BitDepth::Sixteen)
        );
        assert_eq!(sbit, Some(vec![12]));
        assert_eq!(samples, [0xAB, 0xCA, 0x12, 0x31]);
    }

    /// A 12-bit mosaic is demosaiced at its own depth into 16-bit RGB.
    #[test]
    fn bayer12_is_a_16_bit_rgb_png() {
        // A flat 4×4 RGGB mosaic: R = 0xC8F, G = 0x64A, B = 0x323.
        let mut payload = Vec::new();
        for y in 0..4 {
            for x in 0..4 {
                let v: u16 = match (x % 2, y % 2) {
                    (0, 0) => 0xC8F,
                    (1, 1) => 0x323,
                    _ => 0x64A,
                };
                payload.extend_from_slice(&v.to_le_bytes());
            }
        }
        let file = encode_frame(
            &frame(PixelFormat::BayerRG12, 4, 4, &payload),
            ImageFormat::Png,
        )
        .unwrap();
        let (colour, depth, sbit, samples) = read_png(&file);
        assert_eq!(
            (colour, depth),
            (png::ColorType::Rgb, png::BitDepth::Sixteen)
        );
        assert_eq!(sbit, Some(vec![12, 12, 12]));
        // (1, 1) is a blue site with four red diagonals and four green
        // neighbours, all inside the image: R 0xC8F → 0xC8FC,
        // G 0x64A → 0x64A6, B 0x323 → 0x3233.
        let at = (4 + 1) * 3 * 2;
        assert_eq!(&samples[at..at + 6], [0xC8, 0xFC, 0x64, 0xA6, 0x32, 0x33]);
    }

    /// 8- and 16-bit samples go in unchanged, and carry no `sBIT`.
    #[test]
    fn whole_byte_depths_are_written_as_they_are() {
        let file = encode_frame(
            &frame(PixelFormat::Mono8, 2, 1, &[7, 250]),
            ImageFormat::Png,
        )
        .unwrap();
        assert_eq!(
            read_png(&file),
            (
                png::ColorType::Grayscale,
                png::BitDepth::Eight,
                None,
                vec![7, 250]
            )
        );
        let file = encode_frame(
            &frame(PixelFormat::Mono16, 1, 1, &[0x34, 0x12]),
            ImageFormat::Png,
        )
        .unwrap();
        assert_eq!(
            read_png(&file),
            (
                png::ColorType::Grayscale,
                png::BitDepth::Sixteen,
                None,
                vec![0x12, 0x34]
            )
        );
        let file = encode_frame(
            &frame(PixelFormat::BGR8Packed, 1, 1, &[1, 2, 3]),
            ImageFormat::Png,
        )
        .unwrap();
        assert_eq!(
            read_png(&file),
            (
                png::ColorType::Rgb,
                png::BitDepth::Eight,
                None,
                vec![3, 2, 1]
            )
        );
    }

    #[test]
    fn colour_can_be_asked_for() {
        let pixels = viva_pfnc::convert::full_depth(PixelFormat::Mono8, 1, 1, &[9]).unwrap();
        let file = encode(&pixels.into_rgb(), 1, 1, ImageFormat::Png).unwrap();
        assert_eq!(
            read_png(&file),
            (
                png::ColorType::Rgb,
                png::BitDepth::Eight,
                None,
                vec![9, 9, 9]
            )
        );
    }

    /// The same samples as the PNG, in an uncompressed TIFF.
    #[test]
    fn mono12_is_a_16_bit_grey_tiff() {
        let file = encode_frame(
            &frame(PixelFormat::Mono12p, 2, 1, &[0xBC, 0x3A, 0x12]),
            ImageFormat::Tiff,
        )
        .unwrap();
        let mut decoder = tiff::decoder::Decoder::new(Cursor::new(file)).unwrap();
        assert_eq!(decoder.dimensions().unwrap(), (2, 1));
        assert_eq!(decoder.colortype().unwrap(), tiff::ColorType::Gray(16));
        let mut image = tiff::decoder::DecodingResult::U16(Vec::new());
        decoder.read_image_to_buffer(&mut image).unwrap();
        let tiff::decoder::DecodingResult::U16(samples) = image else {
            panic!("16-bit samples expected");
        };
        assert_eq!(samples, [0xABCA, 0x1231]);
    }

    #[test]
    fn rgb_is_an_8_bit_rgb_tiff() {
        let file = encode_frame(
            &frame(PixelFormat::BGR8Packed, 1, 1, &[1, 2, 3]),
            ImageFormat::Tiff,
        )
        .unwrap();
        let mut decoder = tiff::decoder::Decoder::new(Cursor::new(file)).unwrap();
        assert_eq!(decoder.colortype().unwrap(), tiff::ColorType::RGB(8));
        let mut image = tiff::decoder::DecodingResult::U8(Vec::new());
        decoder.read_image_to_buffer(&mut image).unwrap();
        assert!(matches!(image, tiff::decoder::DecodingResult::U8(v) if v == [3, 2, 1]));
    }

    #[test]
    fn the_format_comes_from_the_extension() {
        for (path, format) in [
            ("a.png", Some(ImageFormat::Png)),
            ("dir/b.PNG", Some(ImageFormat::Png)),
            ("c.tif", Some(ImageFormat::Tiff)),
            ("d.tiff", Some(ImageFormat::Tiff)),
            ("e.pgm", None),
            ("f", None),
        ] {
            assert_eq!(ImageFormat::from_path(Path::new(path)), format, "{path}");
        }
    }

    #[test]
    fn an_unconvertible_frame_is_an_error() {
        assert!(matches!(
            encode_frame(
                &frame(PixelFormat::Coord3DC32f, 1, 1, &[0; 4]),
                ImageFormat::Png
            ),
            Err(ImageIoError::Convert(_))
        ));
    }
}
