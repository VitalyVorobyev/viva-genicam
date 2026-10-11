//! A connected camera, whatever carries its registers (backlog ST-14).
//!
//! The library types a camera by its register transport — `Camera<GigeRegisterIo>`
//! for GigE Vision, `Camera<U3vRegisterIo<T>>` for USB3 Vision over any USB
//! transfer `T` — so the embedded backend holds one behind [`Device`], and
//! reads frames from either kind of stream through [`Frames`].

use std::time::Duration;

use viva_genicam::genapi::RegisterIo;
use viva_genicam::u3v::usb::UsbTransfer;
use viva_genicam::{
    Camera, Frame, FrameStream, GenicamError, GigeRegisterIo, StreamOptions, U3vFrameStream,
    U3vRegisterIo, U3vStreamBuilder,
};
use viva_zenoh_api::FeatureState;

use super::StartOptions;

/// What the backend asks of a connected camera. Every method blocks on the
/// camera's I/O, so the backend calls them on the blocking pool.
pub(crate) trait Device: Send {
    /// One feature's live state, as `viva-service` computes it (ST-26).
    fn feature_state(&self, name: &str) -> Result<FeatureState, String>;

    /// Write `value` to `name`, and name the features the write may change.
    /// A command executes on any value.
    fn set(&mut self, name: &str, value: &str) -> Result<Vec<String>, String>;

    /// The image size and pixel format a stream would start with.
    fn image_format(&self) -> (u32, u32, String);

    /// Open the stream and start acquiring.
    fn start(&mut self, options: &StartOptions) -> Result<Frames, String>;

    fn acquisition_stop(&mut self) -> Result<(), String>;

    /// Let go of the camera as far as its transport allows, best-effort: the
    /// caller is disconnecting or exiting, and the camera may be gone.
    fn release(&mut self, ack_timeout: Duration);

    /// The GigE Vision camera, for what only GigE Vision has: an IP address
    /// and control privilege.
    fn gige(&mut self) -> Option<&mut Camera<GigeRegisterIo>> {
        None
    }
}

fn feature_state_of<R: RegisterIo>(camera: &Camera<R>, name: &str) -> Result<FeatureState, String> {
    camera
        .feature_state(name)
        .map(FeatureState::from)
        .map_err(|e| format!("Failed to read '{name}': {e}"))
}

fn set_on<R: RegisterIo>(
    camera: &mut Camera<R>,
    name: &str,
    value: &str,
) -> Result<Vec<String>, String> {
    camera
        .set(name, value)
        .map_err(|e| format!("Failed to write '{name}': {e}"))?;
    Ok(camera.nodemap().invalidated_by(name))
}

fn image_format_of<R: RegisterIo>(camera: &Camera<R>) -> (u32, u32, String) {
    let dimension = |name: &str, fallback: u32| {
        camera
            .get(name)
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(fallback)
    };
    let pixel_format = camera.get("PixelFormat").unwrap_or_else(|error| {
        tracing::warn!(%error, "Failed to read PixelFormat before acquisition");
        "Unknown".to_string()
    });
    (
        dimension("Width", 640),
        dimension("Height", 480),
        pixel_format,
    )
}

impl Device for Camera<GigeRegisterIo> {
    fn feature_state(&self, name: &str) -> Result<FeatureState, String> {
        feature_state_of(self, name)
    }

    fn set(&mut self, name: &str, value: &str) -> Result<Vec<String>, String> {
        set_on(self, name, value)
    }

    fn image_format(&self) -> (u32, u32, String) {
        image_format_of(self)
    }

    fn start(&mut self, options: &StartOptions) -> Result<Frames, String> {
        // The library's one stream-open recipe: claim control again (a
        // release gave it up), route to the camera's interface, negotiate the
        // packet size — a size the user measured for their link is a ceiling
        // the path probe stays under (backlog ST-22, issue #112) — and start
        // the receive engine with the camera's PayloadSize.
        let stream_options = match options.packet_size {
            Some(size) => StreamOptions::new().packet_size(size),
            None => StreamOptions::new().auto_packet_size(),
        };
        let frames = self
            .open_stream(stream_options)
            .map_err(|e| format!("Failed to open the stream: {e}"))?;
        let frames = Frames::Gige(Box::new(frames));
        self.acquisition_start()
            .map_err(|e| format!("Failed to start acquisition: {e}"))?;
        Ok(frames)
    }

    fn acquisition_stop(&mut self) -> Result<(), String> {
        Camera::acquisition_stop(self).map_err(|e| format!("Failed to stop acquisition: {e}"))
    }

    fn release(&mut self, ack_timeout: Duration) {
        if let Ok(mut device) = self.transport().lock_device() {
            device.set_control_timeout(ack_timeout);
        }
        if let Err(error) = Camera::acquisition_stop(self) {
            tracing::debug!(%error, "AcquisitionStop while releasing the camera");
        }
        let handle = tokio::runtime::Handle::current();
        match self.transport().lock_device() {
            Ok(mut device) => {
                if let Err(error) = handle.block_on(device.release_control()) {
                    tracing::debug!(%error, "releasing control privilege");
                }
            }
            Err(error) => tracing::debug!(%error, "releasing control privilege"),
        }
    }

    fn gige(&mut self) -> Option<&mut Camera<GigeRegisterIo>> {
        Some(self)
    }
}

impl<T: UsbTransfer + Send + 'static> Device for Camera<U3vRegisterIo<T>> {
    fn feature_state(&self, name: &str) -> Result<FeatureState, String> {
        feature_state_of(self, name)
    }

    fn set(&mut self, name: &str, value: &str) -> Result<Vec<String>, String> {
        set_on(self, name, value)
    }

    fn image_format(&self) -> (u32, u32, String) {
        image_format_of(self)
    }

    fn start(&mut self, _options: &StartOptions) -> Result<Frames, String> {
        // USB3 Vision has no packet size to set: it streams in bulk transfers.
        let frames = U3vStreamBuilder::new(self)
            .build()
            .map_err(|e| format!("Failed to open the USB stream: {e}"))?;
        self.acquisition_start()
            .map_err(|e| format!("Failed to start acquisition: {e}"))?;
        Ok(Frames::U3v(frames))
    }

    fn acquisition_stop(&mut self) -> Result<(), String> {
        Camera::acquisition_stop(self).map_err(|e| format!("Failed to stop acquisition: {e}"))
    }

    fn release(&mut self, _ack_timeout: Duration) {
        // No control privilege to give back over USB; closing the device when
        // it drops releases the interface.
        if let Err(error) = Camera::acquisition_stop(self) {
            tracing::debug!(%error, "AcquisitionStop while releasing the camera");
        }
    }
}

/// The frames of either kind of stream.
pub(crate) enum Frames {
    Gige(Box<FrameStream>),
    U3v(U3vFrameStream),
}

impl Frames {
    pub(crate) async fn next_frame(&mut self) -> Result<Option<Frame>, GenicamError> {
        match self {
            Frames::Gige(stream) => stream.next_frame().await,
            Frames::U3v(stream) => stream.next_frame().await,
        }
    }

    /// End the stream and wait until its reader has let go of the camera, so
    /// the next stream or a reconnect finds the endpoint free (Codex review
    /// of #179).
    pub(crate) async fn shutdown(self) {
        match self {
            Frames::Gige(stream) => drop(stream),
            Frames::U3v(stream) => stream.shutdown().await,
        }
    }

    /// Frames lost since the stream started: GVSP's closed incomplete, never
    /// seen (block-id gaps) or discarded under backpressure; USB3 Vision's
    /// dropped because their trailer reported an error.
    pub(crate) fn dropped(&self) -> u64 {
        match self {
            Frames::Gige(stream) => stream.stats().frames_lost(),
            Frames::U3v(stream) => stream.dropped(),
        }
    }
}
