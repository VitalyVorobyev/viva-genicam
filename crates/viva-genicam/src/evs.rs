use std::time::SystemTime;

use bytes::Bytes;
use viva_pfnc::PixelFormat;

use crate::chunks::ChunkMap;

/// Encoding used by a LUCID event-vision stream block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EvsFormat {
    /// Prophesee EVT 3.0, encoded as 16-bit words.
    Evt30,
    /// Prophesee EVT 2.1, encoded as 64-bit events.
    Evt21,
}

impl EvsFormat {
    pub(crate) const fn pixel_format(self) -> PixelFormat {
        match self {
            Self::Evt30 => PixelFormat::EvsEvt30,
            Self::Evt21 => PixelFormat::EvsEvt21,
        }
    }
}

/// One complete raw event-vision block reassembled from GVSP packets.
#[derive(Debug, Clone)]
pub struct EvsBlock {
    /// Encoded EVT bytes, preserved exactly as transmitted.
    pub payload: Bytes,
    /// EVT encoding identified by the GVSP leader's format code.
    pub format: EvsFormat,
    /// Device timestamp from the GVSP leader.
    pub ts_dev: Option<u64>,
    /// Raw Size X value from the GVSP leader.
    pub leader_size_x: u32,
    /// Raw Size Y value from the GVSP leader.
    pub leader_size_y: u32,
    /// Raw Size Y value from the GVSP trailer.
    ///
    /// On the captured TRT009S-E stream this equals the encoded payload length,
    /// but it is retained as wire metadata rather than generalized from one
    /// device observation.
    pub trailer_size_y: u32,
    /// Decoded chunk data carried by the trailer, when present.
    pub chunks: Option<ChunkMap>,
    /// Host-correlated timestamp, when time synchronization is available.
    pub ts_host: Option<SystemTime>,
}
