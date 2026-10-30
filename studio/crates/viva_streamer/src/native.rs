//! One source frame, retained only until the next displayed frame. Full-depth
//! decoding and PNG encoding happen only when the user freezes or saves it.

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use serde::Serialize;
use tokio::sync::Semaphore;
use viva_genicam::image_io::{self, ImageFormat};
use viva_pfnc::{
    PixelFormat,
    convert::{self, Pixels},
};
use viva_zenoh_api::{FrameHeader, FrameMetadata, MAX_FRAME_METADATA_BYTES};

/// Prevent a hostile geometry from requesting an unbounded decoded capture.
/// This bounds BMP + raw + decoded + PNG work, independently of the camera.
pub const MAX_CAPTURE_PIXELS: usize = 16 * 1024 * 1024;
const MAX_SOURCE_BYTES: usize = 128 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 256 * 1024 * 1024;

/// The source of the exact BMP currently offered to the viewer.
#[derive(Clone, Debug)]
pub struct SourceFrame {
    pub format: PixelFormat,
    pub width: u32,
    pub height: u32,
    pub row_stride: Option<usize>,
    pub pixels: Bytes,
    pub bmp: Bytes,
    /// Metadata from this exact source, never a separately arriving update.
    pub metadata: Option<FrameMetadata>,
}

/// A single acquisition's source cache and conversion permit. Creating a new
/// server creates a fresh cache, so frames cannot leak across sessions.
#[derive(Clone)]
pub struct NativeCapture {
    latest: Arc<Mutex<Result<Option<SourceFrame>, String>>>,
    pub(crate) permit: Arc<Semaphore>,
}

impl Default for NativeCapture {
    fn default() -> Self {
        Self {
            latest: Arc::new(Mutex::new(Ok(None))),
            permit: Arc::new(Semaphore::new(1)),
        }
    }
}

impl NativeCapture {
    /// Replace the sole retained source. Refuse oversized sources rather than
    /// retaining a frame that can never be captured.
    pub fn replace(&self, frame: SourceFrame) {
        let result = source_size(&frame).map(|_| Some(frame));
        self.store(result);
    }

    /// Retain the explicit Zenoh image layout after preview conversion.
    /// Continuous packing and partial final groups are decoded by PFNC.
    pub fn replace_remote(&self, header: &FrameHeader, pixels: &[u8], bmp: Bytes) {
        let row_stride = header.row_stride.map(|stride| stride as usize);
        // Validate before allocating a copy of a remote payload: a tiny
        // declared image may carry arbitrarily large trailing bytes.
        let result = source_lengths(header.width, header.height, pixels.len(), bmp.len())
            .and_then(|_| {
                remote_layout(
                    header.pixel_format,
                    header.width,
                    header.height,
                    row_stride,
                    pixels.len(),
                )
            })
            .map(|()| {
                Some(SourceFrame {
                    format: header.pixel_format,
                    width: header.width,
                    height: header.height,
                    row_stride,
                    pixels: Bytes::copy_from_slice(pixels),
                    bmp,
                    metadata: header.metadata.clone(),
                })
            });
        self.store(result);
    }

    fn store(&self, result: Result<Option<SourceFrame>, String>) {
        if let Ok(mut latest) = self.latest.lock() {
            *latest = result;
        }
    }

    pub(crate) fn latest(&self) -> Result<Option<SourceFrame>, String> {
        self.latest
            .lock()
            .map_err(|_| "Native capture cache is unavailable".to_string())?
            .clone()
    }
}

fn capture_size(width: u32, height: u32) -> Result<usize, String> {
    let count = usize::try_from(width)
        .ok()
        .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
        .filter(|&count| count > 0 && count <= MAX_CAPTURE_PIXELS)
        .ok_or_else(|| "Native capture exceeds the supported image size".to_string())?;
    Ok(count)
}

fn remote_layout(
    format: PixelFormat,
    width: u32,
    height: u32,
    row_stride: Option<usize>,
    payload_len: usize,
) -> Result<(), String> {
    if let Some(stride) = row_stride {
        let line = format
            .line_bytes(width)
            .ok_or_else(|| "Native capture cannot size this pixel format".to_string())?;
        let expected = stride
            .checked_mul(height as usize)
            .ok_or_else(|| "Native capture row layout overflows".to_string())?;
        if stride < line || payload_len < expected {
            return Err(
                "Native capture has an invalid row stride or incomplete padded rows".to_string(),
            );
        }
    }
    // No stride means one continuous packed image. The preview and native
    // converters own its minimum length, including partial final groups.
    Ok(())
}

fn source_size(frame: &SourceFrame) -> Result<usize, String> {
    source_lengths(
        frame.width,
        frame.height,
        frame.pixels.len(),
        frame.bmp.len(),
    )
}

fn source_lengths(width: u32, height: u32, pixels: usize, bmp: usize) -> Result<usize, String> {
    let count = capture_size(width, height)?;
    if pixels
        .checked_add(bmp)
        .is_none_or(|size| size > MAX_SOURCE_BYTES)
    {
        return Err("Native capture source exceeds 128 MiB".to_string());
    }
    Ok(count)
}

fn response_size(lengths: &[usize]) -> Result<usize, String> {
    lengths
        .iter()
        .try_fold(0usize, |total, &length| total.checked_add(length))
        .filter(|&size| size <= MAX_RESPONSE_BYTES)
        .ok_or_else(|| "Native capture response exceeds 256 MiB".to_string())
}

#[derive(Serialize)]
struct Header<'a> {
    version: u8,
    width: u32,
    height: u32,
    pixel_format: String,
    bits: u8,
    channels: u8,
    mosaic: bool,
    bmp_bytes: usize,
    sample_bytes: usize,
    png_bytes: usize,
    metadata: Option<&'a FrameMetadata>,
}

/// VRAW, a little-endian u32 JSON-header length, UTF-8 metadata, BMP, native
/// samples (u8 or little-endian u16), then a full-depth PNG. The BMP, samples
/// and PNG always describe the same immutable source.
pub fn encode(frame: &SourceFrame) -> Result<Bytes, String> {
    let count = source_size(frame)?;
    let metadata = frame
        .metadata
        .clone()
        .map(crate::preview::normalize)
        .transpose()?;
    let full = convert::full_depth(
        frame.format,
        frame.width,
        frame.height,
        frame.row_stride,
        &frame.pixels,
    )
    .map_err(|error| error.to_string())?;
    let png = image_io::encode(&full, frame.width, frame.height, ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    let mosaic = frame.format.is_bayer();
    let (bits, channels, samples) = match &full {
        Pixels::Gray8(values) => (8, 1, values.clone()),
        Pixels::Gray16 { samples, bits } => (*bits, 1, little_endian(samples)),
        Pixels::Rgb8(values) if !mosaic => (8, 3, values.clone()),
        Pixels::Rgb16 { samples, bits } if !mosaic => (*bits, 3, little_endian(samples)),
        Pixels::Rgb8(_) | Pixels::Rgb16 { .. } if mosaic => {
            let values = convert::unpack_u16(
                frame.format,
                frame.width,
                frame.height,
                frame.row_stride,
                &frame.pixels,
            )
            .map_err(|error| error.to_string())?;
            let bits = match &full {
                Pixels::Rgb16 { bits, .. } => *bits,
                _ => 8,
            };
            let samples = if bits == 8 {
                values.into_iter().map(|v| v as u8).collect()
            } else {
                little_endian(&values)
            };
            (bits, 1, samples)
        }
        _ => return Err("Unsupported native sample layout".to_string()),
    };
    let expected = count
        .checked_mul(usize::from(channels))
        .and_then(|n| n.checked_mul(if bits > 8 { 2 } else { 1 }))
        .ok_or_else(|| "Native sample size overflow".to_string())?;
    if samples.len() != expected {
        return Err("Native sample geometry mismatch".to_string());
    }
    let header = serde_json::to_vec(&Header {
        version: 1,
        width: frame.width,
        height: frame.height,
        pixel_format: frame.format.to_string(),
        bits,
        channels,
        mosaic,
        bmp_bytes: frame.bmp.len(),
        sample_bytes: samples.len(),
        png_bytes: png.len(),
        metadata: metadata.as_ref(),
    })
    .map_err(|error| error.to_string())?;
    if header.len() > MAX_FRAME_METADATA_BYTES + 4096 {
        return Err("Native capture metadata exceeds the header limit".into());
    }
    let mut output = Vec::with_capacity(response_size(&[
        8,
        header.len(),
        frame.bmp.len(),
        samples.len(),
        png.len(),
    ])?);
    output.extend_from_slice(b"VRAW");
    output.extend_from_slice(&(header.len() as u32).to_le_bytes());
    output.extend_from_slice(&header);
    output.extend_from_slice(&frame.bmp);
    output.extend_from_slice(&samples);
    output.extend_from_slice(&png);
    Ok(output.into())
}

fn little_endian(values: &[u16]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::FrameEncoder;

    fn header(format: PixelFormat, width: u32, height: u32, stride: Option<usize>) -> FrameHeader {
        FrameHeader {
            pixel_format: format,
            width,
            height,
            row_stride: stride.map(|s| s as u32),
            seq: 1,
            metadata: None,
        }
    }

    fn source(format: PixelFormat, width: u32, height: u32, pixels: &[u8]) -> SourceFrame {
        SourceFrame {
            format,
            width,
            height,
            row_stride: None,
            metadata: None,
            pixels: Bytes::copy_from_slice(pixels),
            bmp: FrameEncoder::new()
                .encode(format, width, height, None, pixels)
                .unwrap(),
        }
    }

    #[test]
    fn capture_retains_native_twelve_bit_values_and_png_significant_bits() {
        let source = source(PixelFormat::Mono12, 2, 1, &[0xBC, 0x0A, 0xFF, 0x0F]);
        let encoded = encode(&source).unwrap();
        let header_len = u32::from_le_bytes(encoded[4..8].try_into().unwrap()) as usize;
        let header: serde_json::Value =
            serde_json::from_slice(&encoded[8..8 + header_len]).unwrap();
        assert_eq!(header["bits"], 12);
        let start = 8 + header_len + source.bmp.len();
        assert_eq!(&encoded[start..start + 4], &[0xBC, 0x0A, 0xFF, 0x0F]);
        let png = &encoded[start + 4..];
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert!(png.windows(5).any(|bytes| bytes == b"sBIT\x0c"));
    }

    #[test]
    fn bayer_analysis_is_the_sensor_mosaic_and_not_demosaiced_rgb() {
        let source = source(PixelFormat::BayerRG12, 2, 2, &[1, 0, 2, 0, 3, 0, 4, 0]);
        let encoded = encode(&source).unwrap();
        let len = u32::from_le_bytes(encoded[4..8].try_into().unwrap()) as usize;
        let header: serde_json::Value = serde_json::from_slice(&encoded[8..8 + len]).unwrap();
        assert_eq!(header["channels"], 1);
        assert_eq!(header["mosaic"], true);
        let start = 8 + len + source.bmp.len();
        assert_eq!(&encoded[start..start + 8], source.pixels.as_ref());
    }

    #[test]
    fn frozen_metadata_and_pixels_remain_together_after_a_new_frame_arrives() {
        let facts: FrameMetadata = serde_json::from_value(serde_json::json!({
            "block_id": "18446744073709551615", "ts_dev": "9007199254740993",
            "ts_host_ns": null, "complete": true, "chunks": [], "features": [],
            "chunk_error": null, "truncated": false
        }))
        .unwrap();
        let mut first = source(PixelFormat::Mono8, 1, 1, &[17]);
        first.metadata = Some(facts.clone());
        let cache = NativeCapture::default();
        cache.replace(first);
        let frozen = cache.latest().unwrap().unwrap();
        cache.replace(source(PixelFormat::Mono8, 1, 1, &[99]));
        let encoded = encode(&frozen).unwrap();
        let length = u32::from_le_bytes(encoded[4..8].try_into().unwrap()) as usize;
        let header: serde_json::Value = serde_json::from_slice(&encoded[8..8 + length]).unwrap();
        assert_eq!(header["metadata"], serde_json::to_value(facts).unwrap());
        assert_eq!(samples(&encoded), &[17]);
        assert!(cache.latest().unwrap().unwrap().metadata.is_none());
    }

    #[test]
    fn malformed_or_oversized_images_are_refused_and_old_sources_are_not_retained() {
        assert!(capture_size(u32::MAX, u32::MAX).is_err());
        assert!(capture_size(0, 1).is_err());
        let mut frame = source(PixelFormat::Mono8, 1, 1, &[7]);
        frame.pixels = Bytes::new();
        assert!(encode(&frame).is_err());
        let cache = NativeCapture::default();
        cache.replace(source(PixelFormat::Mono8, 1, 1, &[1]));
        assert!(cache.latest().unwrap().is_some());
        let fresh_session = NativeCapture::default();
        assert!(fresh_session.latest().unwrap().is_none());
        frame.width = u32::MAX;
        cache.replace(frame);
        assert!(cache.latest().is_err());
    }

    #[test]
    fn oversized_trailing_sources_and_container_length_overflows_are_refused() {
        let cache = NativeCapture::default();
        cache.replace(source(PixelFormat::Mono8, 1, 1, &[7]));
        let mut frame = source(PixelFormat::Mono8, 1, 1, &[1]);
        // The zeroed allocation is never touched by the decoder: the input
        // byte bound must reject it before conversion, even at 1×1 geometry.
        frame.pixels = vec![0; MAX_SOURCE_BYTES].into();
        assert!(encode(&frame).unwrap_err().contains("128 MiB"));
        cache.replace(frame);
        assert!(cache.latest().is_err());
        assert!(response_size(&[usize::MAX, 1]).is_err());
        assert!(response_size(&[MAX_RESPONSE_BYTES, 1]).is_err());
        let mut unsupported = source(PixelFormat::Mono8, 1, 1, &[7]);
        unsupported.format = PixelFormat::Unknown(0);
        assert!(encode(&unsupported).is_err());
    }

    fn samples(encoded: &[u8]) -> &[u8] {
        let len = u32::from_le_bytes(encoded[4..8].try_into().unwrap()) as usize;
        let header: serde_json::Value = serde_json::from_slice(&encoded[8..8 + len]).unwrap();
        let start = 8 + len + header["bmp_bytes"].as_u64().unwrap() as usize;
        let length = header["sample_bytes"].as_u64().unwrap() as usize;
        &encoded[start..start + length]
    }

    #[test]
    fn remote_padded_rows_preserve_preview_and_native_values() {
        let pixels = [1, 2, 99, 98, 3, 4, 97, 96];
        let cache = NativeCapture::default();
        let bmp = FrameEncoder::new()
            .encode(PixelFormat::Mono8, 2, 2, Some(4), &pixels)
            .unwrap();
        let expected = source(PixelFormat::Mono8, 2, 2, &[1, 2, 3, 4]);
        assert_eq!(bmp, expected.bmp);
        assert_ne!(
            bmp,
            FrameEncoder::new()
                .encode(PixelFormat::Mono8, 2, 2, None, &pixels)
                .unwrap()
        );
        cache.replace_remote(
            &header(PixelFormat::Mono8, 2, 2, Some(4)),
            &pixels,
            bmp.clone(),
        );
        let retained = cache.latest().unwrap().unwrap();
        assert_eq!(retained.bmp, bmp);
        assert_eq!(samples(&encode(&retained).unwrap()), &[1, 2, 3, 4]);
    }

    #[test]
    fn remote_continuous_packed_partial_group_preserves_native_values() {
        // Mono12Packed pairs: high byte, shared low nibbles, high byte.
        let odd = source(PixelFormat::Mono12Packed, 3, 1, &[1, 0x42, 3, 5, 6]);
        let cache = NativeCapture::default();
        cache.replace_remote(
            &header(odd.format, odd.width, odd.height, None),
            &odd.pixels,
            odd.bmp.clone(),
        );
        let retained = cache.latest().unwrap().unwrap();
        assert_eq!(retained.bmp, odd.bmp);
        assert_eq!(
            samples(&encode(&retained).unwrap()),
            little_endian(&[0x12, 0x34, 0x56])
        );
    }

    #[test]
    fn remote_invalid_strides_clear_previous_capture_without_copying() {
        let cache = NativeCapture::default();
        for stride in [Some(1), Some(4), Some(usize::MAX)] {
            cache.replace(source(PixelFormat::Mono8, 2, 2, &[1, 2, 3, 4]));
            cache.replace_remote(
                &header(PixelFormat::Mono8, 2, 2, stride),
                &[1, 2, 3, 4],
                Bytes::new(),
            );
            assert!(cache.latest().is_err());
        }
        cache.replace_remote(
            &header(PixelFormat::Mono8, 1, 1, None),
            &vec![0; MAX_SOURCE_BYTES + 1],
            Bytes::new(),
        );
        assert!(cache.latest().unwrap_err().contains("128 MiB"));
    }
}
