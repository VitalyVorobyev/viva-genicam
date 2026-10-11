//! Frame representation combining pixel data with optional chunk metadata.

use std::time::SystemTime;

use bytes::Bytes;
use viva_pfnc::PixelFormat;
use viva_pfnc::convert::ConvertError;

use crate::chunks::{ChunkKind, ChunkMap, ChunkValue};

/// How a frame's block ended.
///
/// A stream hands out only [`FrameStatus::Complete`] frames unless it was asked
/// to deliver incomplete ones too (`FrameStream::set_deliver_incomplete`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FrameStatus {
    /// Every packet arrived and the device reported success.
    Complete,
    /// The trailer arrived; `missing_packets` packets (the leader among them,
    /// if it was lost) did not. Their bytes in the payload are zero.
    Incomplete {
        /// Packets that never arrived.
        missing_packets: u32,
    },
    /// The trailer reported this error status (bit 15 set).
    DeviceStatus(u16),
    /// No trailer arrived before the block timed out or was displaced, so how
    /// many packets the block had is unknown.
    TrailerMissing,
}

impl FrameStatus {
    /// Whether this is [`FrameStatus::Complete`].
    pub fn is_complete(self) -> bool {
        matches!(self, FrameStatus::Complete)
    }
}

/// Image frame produced by the GigE Vision stream reassembler.
///
/// Construct one with [`Frame::new`]; the struct is `#[non_exhaustive]` so the
/// stream can report more about a block without breaking callers.
#[derive(Debug, Clone)]
#[non_exhaustive]
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
    /// Block id from the transport: 16 bits widened, or the 64-bit extended id.
    pub block_id: u64,
    /// Payload type from the leader, all 16 bits: `0x0001` is an image and
    /// `0x4001` an image with extended chunk data (TC-05).
    pub payload_type: u16,
    /// How the block ended.
    pub status: FrameStatus,
}

/// Payload type of an image block.
const PAYLOAD_TYPE_IMAGE: u16 = 0x0001;

impl Frame {
    /// A complete image frame with no chunks, timestamps or block id.
    pub fn new(payload: Bytes, width: u32, height: u32, pixel_format: PixelFormat) -> Self {
        Self {
            payload,
            width,
            height,
            pixel_format,
            chunks: None,
            ts_dev: None,
            ts_host: None,
            block_id: 0,
            payload_type: PAYLOAD_TYPE_IMAGE,
            status: FrameStatus::Complete,
        }
    }

    /// Whether every packet of the frame arrived.
    pub fn is_complete(&self) -> bool {
        self.status.is_complete()
    }

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
    ///
    /// Every format [`viva_pfnc::convert`] handles: monochrome and Bayer at 8
    /// to 16 bits, packed or not, RGB/BGR with or without alpha, and 8-bit
    /// Y'CbCr. Samples deeper than 8 bits keep their top eight bits, and a
    /// Bayer mosaic is demosaiced bilinearly — the same pixels Viva Studio's
    /// viewer shows. Bytes past the image are ignored.
    pub fn to_rgb8(&self) -> Result<Vec<u8>, crate::GenicamError> {
        viva_pfnc::convert::to_rgb8(self.pixel_format, self.width, self.height, &self.payload)
            .map_err(|err| match err {
                ConvertError::Unsupported(format) => {
                    crate::GenicamError::UnsupportedPixelFormat(format)
                }
                other => crate::GenicamError::Parse(other.to_string()),
            })
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
        Frame::new(Bytes::copy_from_slice(payload), width, height, pixel_format)
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

    /// `to_rgb8` is `viva_pfnc::convert::to_rgb8` — the conversion Viva
    /// Studio's viewer draws with — for every family of format it handles.
    #[test]
    fn to_rgb8_is_the_shared_conversion() {
        use viva_pfnc::convert;
        let mosaic: Vec<u8> = (0..36u8).map(|v| v.wrapping_mul(37)).collect();
        let cases: [(PixelFormat, u32, u32, Vec<u8>); 9] = [
            (PixelFormat::BayerRG8, 6, 6, mosaic.clone()),
            (PixelFormat::BayerGB8, 6, 6, mosaic.clone()),
            (PixelFormat::Mono8, 6, 6, mosaic),
            (PixelFormat::Mono16, 2, 1, vec![0x34, 0x12, 0xFF, 0xFF]),
            (PixelFormat::Mono12Packed, 2, 1, vec![0xAB, 0x3C, 0x12]),
            (
                PixelFormat::BayerRG12p,
                2,
                2,
                vec![0xBC, 0x3A, 0x12, 0x45, 0x06, 0x78],
            ),
            (PixelFormat::YUV422_8, 2, 1, vec![100, 150, 200, 90]),
            (PixelFormat::BGR8Packed, 1, 1, vec![1, 2, 3]),
            (PixelFormat::RGBa8, 1, 1, vec![1, 2, 3, 4]),
        ];
        for (format, width, height, payload) in cases {
            let frame = frame_with_payload(&payload, width, height, format);
            assert_eq!(
                frame.to_rgb8().ok(),
                convert::to_rgb8(format, width, height, &payload).ok(),
                "{format}"
            );
        }
    }

    #[test]
    fn to_rgb8_reports_a_short_payload_and_an_unconvertible_format() {
        let short = frame_with_payload(&[1, 2, 3], 2, 2, PixelFormat::Mono8);
        assert!(matches!(
            short.to_rgb8(),
            Err(crate::GenicamError::Parse(_))
        ));
        let depth = frame_with_payload(&[0; 4], 1, 1, PixelFormat::Coord3DC32f);
        assert!(matches!(
            depth.to_rgb8(),
            Err(crate::GenicamError::UnsupportedPixelFormat(
                PixelFormat::Coord3DC32f
            ))
        ));
    }
}
