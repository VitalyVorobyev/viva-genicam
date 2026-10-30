//! What the acquisition queryable needs of a camera: a stream of blocks.

use viva_genicam::report::ReportTarget;
use viva_genicam::{
    Event, EventConfiguration, EventStream, FrameStream, GenicamError, StreamBlock, StreamStats,
};

use crate::device::DeviceOps;

/// A camera's open stream, as the frame loop reads it.
#[async_trait::async_trait]
pub trait BlockStream: Send {
    /// The next block: an image, or an event-vision block. `Ok(None)` when
    /// the stream has ended.
    async fn next_block(&mut self) -> Result<Option<StreamBlock>, GenicamError>;

    /// The stream's counters so far, for the acquisition status. A stream
    /// that keeps none reports `None`.
    fn stats(&self) -> Option<StreamStats> {
        None
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

    fn stats(&self) -> Option<StreamStats> {
        Some(FrameStream::stats(self))
    }
}

/// A camera's open event channel, as the event loop reads it.
#[async_trait::async_trait]
pub trait EventReceiver: Send + Sync {
    /// The next event; an error once the channel has failed.
    async fn next_event(&self) -> Result<Event, GenicamError>;

    /// Stop receiving and let go of what the receiver holds; an error when
    /// that could not be undone on the device, such as a USB3 Vision
    /// EventEnable left set.
    async fn close(self: Box<Self>) -> Result<(), GenicamError> {
        Ok(())
    }
}

/// A GigE Vision message channel: closing it is dropping its socket.
#[async_trait::async_trait]
impl EventReceiver for EventStream {
    async fn next_event(&self) -> Result<Event, GenicamError> {
        self.next().await
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

    /// Configure the camera to send the `EventSelector` entries `events`
    /// and open the channel they arrive on. The configuration token is
    /// restored with [`viva_genicam::Camera::restore_events`] once the
    /// receiver is closed. A setup error says what its rollback could not
    /// restore.
    async fn open_events(
        &self,
        events: Vec<String>,
    ) -> Result<(Box<dyn EventReceiver>, EventConfiguration), String>;

    /// Whether this transport's events carry a stream channel and block id:
    /// GigE Vision's do, USB3 Vision's do not.
    fn events_carry_blocks(&self) -> bool {
        true
    }
}
