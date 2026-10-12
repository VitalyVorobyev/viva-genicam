//! What the acquisition queryable needs of a camera: a stream of blocks.

use viva_genicam::report::ReportTarget;
use viva_genicam::{FrameStream, GenicamError, StreamBlock};

use crate::device::DeviceOps;

/// A camera's open stream, as the frame loop reads it.
#[async_trait::async_trait]
pub trait BlockStream: Send {
    /// The next block: an image, or an event-vision block. `Ok(None)` when
    /// the stream has ended.
    async fn next_block(&mut self) -> Result<Option<StreamBlock>, GenicamError>;

    /// Frames lost so far, for the acquisition status. A transport that
    /// counts none reports 0.
    fn frames_lost(&self) -> u64 {
        0
    }

    /// Stop the stream and let go of what it holds.
    async fn close(self: Box<Self>) {}
}

/// A GigE Vision stream: GVSP through the receive engine (ADR-0025).
#[async_trait::async_trait]
impl BlockStream for FrameStream {
    async fn next_block(&mut self) -> Result<Option<StreamBlock>, GenicamError> {
        Ok(FrameStream::next_block(self).await?.map(StreamBlock::from))
    }

    fn frames_lost(&self) -> u64 {
        self.stats().frames_lost()
    }
}

/// A camera a service streams from, over whichever transport: the one
/// abstraction the GigE Vision and USB3 Vision services share for
/// acquisition, so their Zenoh behaviour is one implementation (backlog
/// API-04).
#[async_trait::async_trait]
pub trait StreamSource: DeviceOps {
    /// Open the camera's stream, ready for `AcquisitionStart`.
    ///
    /// `packet_size` is GigE Vision's ceiling for the path probe; a transport
    /// without packets ignores it.
    async fn open_stream(
        &self,
        packet_size: Option<u32>,
    ) -> Result<Box<dyn BlockStream>, GenicamError>;

    /// What the diagnostic report describes: the camera by address, for one
    /// the report can open itself, or what the service read of it, for one
    /// it holds exclusively.
    async fn report_target(&self) -> ReportTarget;

    /// The service serving the camera, as the report names it, such as
    /// `"viva-service"`.
    fn service_name(&self) -> &'static str;
}
