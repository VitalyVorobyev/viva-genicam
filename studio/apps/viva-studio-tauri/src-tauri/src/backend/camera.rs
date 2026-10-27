//! A connected camera, whatever carries its registers (backlog ST-14).
//!
//! The library types a camera by its register transport — `Camera<GigeRegisterIo>`
//! for GigE Vision, `Camera<U3vRegisterIo<T>>` for USB3 Vision over any USB
//! transfer `T` — so the embedded backend holds one behind [`Device`], and
//! reads frames from either kind of stream through [`Frames`].

use std::time::Duration;

use viva_genicam::genapi::RegisterIo;
use viva_genicam::u3v::UsbTransfer;
use viva_genicam::{
    COMMAND_TIMEOUT, Camera, ControlState, Frame, FrameStream, GenicamError, GigeRegisterIo,
    StreamOptions, StreamStats, U3vFrameStream, U3vRegisterIo, U3vStreamOptions,
};
use viva_zenoh_api::FeatureState;

use super::StartOptions;

/// What the backend asks of a connected camera. Every method blocks on the
/// camera's I/O, so the backend calls them on the blocking pool.
pub(crate) trait Device: Send {
    /// A transport health read; never a claim or a write. Runs on the blocking pool.
    fn health(&mut self) -> Result<ControlState, String>;

    fn watch_control(&self) -> Option<tokio::sync::watch::Receiver<ControlState>> {
        None
    }

    /// One feature's live state, as `viva-service` computes it (ST-26).
    fn feature_state(&self, name: &str) -> Result<FeatureState, String>;

    /// Write `value` to `name`, and name the features the write may change.
    fn set(&mut self, name: &str, value: &str) -> Result<Vec<String>, String>;

    /// Execute command `name`, waiting for the camera to report it finished
    /// (GenICam `IsDone`, up to [`COMMAND_TIMEOUT`]), and name the features
    /// it may change.
    fn execute(&mut self, name: &str) -> Result<Vec<String>, String>;

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

/// A `UserSetSave` is finished when the button returns (backlog ST-48). A
/// command still busy at the timeout was executed all the same: logged.
fn execute_on<R: RegisterIo>(camera: &mut Camera<R>, name: &str) -> Result<Vec<String>, String> {
    match camera.execute_and_wait_blocking(name, COMMAND_TIMEOUT) {
        Ok(()) => {}
        Err(GenicamError::CommandNotDone { waited, .. }) => {
            tracing::warn!(name, ?waited, "command executed but not reported done");
        }
        Err(e) => return Err(format!("Failed to execute '{name}': {e}")),
    }
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
    fn health(&mut self) -> Result<ControlState, String> {
        let state = self.transport().control_state();
        let timeout = {
            let mut device = self.transport().lock_device().map_err(|e| e.to_string())?;
            let timeout = device.control_timeout();
            device.set_control_timeout(Duration::from_millis(250));
            timeout
        };
        let result = if state == ControlState::Held {
            // This shared facade probe records an exclusive takeover as Lost,
            // stops its keepalive, and prevents Drop from releasing its owner.
            self.transport()
                .refresh_control_state()
                .map_err(|e| e.to_string())
        } else {
            let mut device = self.transport().lock_device().map_err(|e| e.to_string())?;
            match tokio::runtime::Handle::current().block_on(device.read_register_or_mem(0, 4)) {
                Ok(_) => Ok(state),
                Err(viva_genicam::gige::GigeError::Status(
                    viva_genicam::gige::GigeStatus::Common(
                        viva_genicam::gencp::StatusCode::AccessDenied,
                    ),
                )) => Ok(state),
                Err(error) => Err(error.to_string()),
            }
        };
        self.transport()
            .lock_device()
            .map_err(|e| e.to_string())?
            .set_control_timeout(timeout);
        result
    }

    fn watch_control(&self) -> Option<tokio::sync::watch::Receiver<ControlState>> {
        Some(self.transport().watch_control())
    }

    fn feature_state(&self, name: &str) -> Result<FeatureState, String> {
        feature_state_of(self, name)
    }

    fn set(&mut self, name: &str, value: &str) -> Result<Vec<String>, String> {
        set_on(self, name, value)
    }

    fn execute(&mut self, name: &str) -> Result<Vec<String>, String> {
        execute_on(self, name)
    }

    fn image_format(&self) -> (u32, u32, String) {
        image_format_of(self)
    }

    fn start(&mut self, options: &StartOptions) -> Result<Frames, String> {
        if self.transport().control_state() != ControlState::Held {
            return Err("Control lost: reconnect explicitly before starting acquisition".into());
        }
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
        if self.transport().control_state() != ControlState::Held {
            return;
        }
        if let Ok(mut device) = self.transport().lock_device() {
            device.set_control_timeout(ack_timeout);
        }
        if let Err(error) = Camera::acquisition_stop(self) {
            tracing::debug!(%error, "AcquisitionStop while releasing the camera");
        }
        // Dropping the camera releases it too; here it happens under the
        // caller's shorter acknowledgement timeout, and dropping then finds
        // nothing left to release.
        if let Err(error) = self.transport().release_control() {
            tracing::debug!(%error, "releasing control privilege");
        }
    }

    fn gige(&mut self) -> Option<&mut Camera<GigeRegisterIo>> {
        Some(self)
    }
}

impl<T: UsbTransfer + Send + 'static> Device for Camera<U3vRegisterIo<T>> {
    fn health(&mut self) -> Result<ControlState, String> {
        // GenCP version is the first mandatory ABRM word. This uncached read
        // also detects an idle unplug, unlike the cached is_lost flag alone.
        if self.transport().is_lost() {
            return Err("USB3 Vision camera disconnected".into());
        }
        self.transport().read(0, 4).map_err(|e| e.to_string())?;
        Ok(ControlState::Held)
    }

    fn feature_state(&self, name: &str) -> Result<FeatureState, String> {
        feature_state_of(self, name)
    }

    fn set(&mut self, name: &str, value: &str) -> Result<Vec<String>, String> {
        set_on(self, name, value)
    }

    fn execute(&mut self, name: &str) -> Result<Vec<String>, String> {
        execute_on(self, name)
    }

    fn image_format(&self) -> (u32, u32, String) {
        image_format_of(self)
    }

    fn start(&mut self, _options: &StartOptions) -> Result<Frames, String> {
        // USB3 Vision has no packet size to set: it streams in bulk transfers.
        let frames = Camera::<U3vRegisterIo<T>>::open_stream(self, U3vStreamOptions::default())
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
    #[cfg(test)]
    Ended,
}

impl Frames {
    pub(crate) async fn next_frame(&mut self) -> Result<Option<Frame>, GenicamError> {
        match self {
            Frames::Gige(stream) => stream.next_frame().await,
            Frames::U3v(stream) => stream.next_frame().await,
            #[cfg(test)]
            Frames::Ended => Ok(None),
        }
    }

    /// End the stream and wait until its reader has let go of the camera, so
    /// the next stream or a reconnect finds the endpoint free (Codex review
    /// of #179).
    pub(crate) async fn shutdown(self) {
        match self {
            Frames::Gige(stream) => drop(stream),
            Frames::U3v(stream) => stream.shutdown().await,
            #[cfg(test)]
            Frames::Ended => {}
        }
    }

    /// The stream's counters since it started, one shape for both
    /// transports.
    pub(crate) fn stats(&self) -> StreamStats {
        match self {
            Frames::Gige(stream) => stream.stats(),
            Frames::U3v(stream) => stream.stats(),
            #[cfg(test)]
            Frames::Ended => StreamStats::default(),
        }
    }
}
