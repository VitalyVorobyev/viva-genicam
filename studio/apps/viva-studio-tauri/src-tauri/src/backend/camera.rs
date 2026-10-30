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
    COMMAND_TIMEOUT, Camera, ControlState, Event, EventConfiguration, EventRestoreReport,
    EventSetupError, EventStream, Frame, FrameStream, GenicamError, GigeRegisterIo, StreamOptions,
    StreamStats, U3vEventStream, U3vFrameStream, U3vRegisterIo, U3vStreamOptions,
};
use viva_zenoh_api::FeatureState;

use super::StartOptions;

/// An event's `EventSelector` entry, if the XML names one, and each feature
/// its port feeds with its value or read error.
pub(crate) type ReadEvent = (Option<String>, Vec<(String, Result<String, String>)>);

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

    /// Decode this frame's attached chunk ports without device register I/O.
    fn chunk_features(&mut self, _frame: &Frame) -> Vec<(String, Result<String, String>)> {
        Vec::new()
    }

    /// Open the stream and start acquiring.
    fn start(&mut self, options: &StartOptions) -> Result<Frames, String>;

    /// Turn notification on for the `EventSelector` entries `names` and open
    /// the channel their events arrive on. The token restores what this
    /// changed ([`Device::restore_events`]) once the channel is shut down.
    fn start_events(
        &mut self,
        names: &[String],
    ) -> Result<(CameraEvents, EventConfiguration), String>;

    /// Put back the event settings [`Device::start_events`] changed, as far
    /// as the session still allows.
    fn restore_events(&mut self, saved: EventConfiguration) -> EventRestoreReport;

    /// The `EventSelector` entry `event` belongs to, and the features its
    /// port feeds; reads the event's own bytes, not the device.
    fn read_event(&mut self, event: &Event) -> ReadEvent;

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

fn read_event_of<R: RegisterIo>(camera: &mut Camera<R>, event: &Event) -> ReadEvent {
    let name = camera.event_name(event.id);
    let features = camera
        .event_features(event)
        .into_iter()
        .map(|(name, value)| (name, value.map_err(|error| error.to_string())))
        .collect();
    (name, features)
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

    fn chunk_features(&mut self, frame: &Frame) -> Vec<(String, Result<String, String>)> {
        Camera::chunk_features(self, frame)
            .into_iter()
            .map(|(name, value)| (name, value.map_err(|error| error.to_string())))
            .collect()
    }

    fn start(&mut self, options: &StartOptions) -> Result<Frames, String> {
        let monitor = self.transport().control_state() == ControlState::Monitor;
        if !monitor && self.transport().control_state() != ControlState::Held {
            return Err("Control lost: reconnect explicitly before starting acquisition".into());
        }
        // The library's one stream-open recipe: claim control again (a
        // release gave it up), route to the camera's interface, negotiate the
        // packet size — a size the user measured for their link is a ceiling
        // the path probe stays under (backlog ST-22, issue #112) — and start
        // the receive engine with the camera's PayloadSize.
        // Use the host address of the actual control socket. Discovery may
        // have selected one of several interfaces sharing the camera's subnet.
        let local = self
            .transport()
            .lock_device()
            .map_err(|e| e.to_string())?
            .local_addr()
            .map_err(|e| format!("Stream interface: {e}"))?
            .ip();
        let std::net::IpAddr::V4(local) = local else {
            return Err("GigE Vision streaming requires an IPv4 interface".into());
        };
        let iface = viva_genicam::gige::Iface::from_ipv4(local)
            .map_err(|e| format!("Stream interface {local}: {e}"))?;
        let stream_options = if monitor {
            StreamOptions::monitor()
        } else {
            match options.packet_size {
                Some(size) => StreamOptions::new().packet_size(size),
                None => StreamOptions::new().auto_packet_size(),
            }
        };
        let frames = self
            .open_stream(stream_options.iface(iface))
            .map_err(|e| format!("Failed to open the stream: {e}"))?;
        let frames = Frames::Gige(Box::new(frames));
        if !monitor {
            self.acquisition_start()
                .map_err(|e| format!("Failed to start acquisition: {e}"))?;
        }
        Ok(frames)
    }

    fn start_events(
        &mut self,
        names: &[String],
    ) -> Result<(CameraEvents, EventConfiguration), String> {
        let local = self
            .transport()
            .lock_device()
            .map_err(|e| e.to_string())?
            .local_addr()
            .map_err(|e| format!("Event interface: {e}"))?
            .ip();
        let std::net::IpAddr::V4(local) = local else {
            return Err("GigE Vision events require an IPv4 interface".into());
        };
        // On the blocking pool: the socket bind needs the runtime, and the
        // register transport drives its own I/O from this thread.
        let runtime = tokio::runtime::Handle::current();
        let events = runtime
            .block_on(self.open_event_stream(local, 0))
            .map_err(|e| format!("Failed to open the event channel: {e}"))?;
        let port = events
            .local_addr()
            .map_err(|e| format!("Event channel: {e}"))?
            .port();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let saved = runtime
            .block_on(self.configure_events_restorable(local, port, &names))
            .map_err(|error| viva_zenoh_api::setup_error_message(&error))?;
        Ok((CameraEvents::Gige(Box::new(events)), saved))
    }

    fn restore_events(&mut self, saved: EventConfiguration) -> EventRestoreReport {
        Camera::restore_events(self, saved)
    }

    fn read_event(&mut self, event: &Event) -> ReadEvent {
        read_event_of(self, event)
    }

    fn acquisition_stop(&mut self) -> Result<(), String> {
        if self.transport().control_state() == ControlState::Monitor {
            return Ok(());
        }
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

    fn chunk_features(&mut self, frame: &Frame) -> Vec<(String, Result<String, String>)> {
        Camera::chunk_features(self, frame)
            .into_iter()
            .map(|(name, value)| (name, value.map_err(|error| error.to_string())))
            .collect()
    }

    fn start(&mut self, _options: &StartOptions) -> Result<Frames, String> {
        // USB3 Vision has no packet size to set: it streams in bulk transfers.
        let frames = Camera::<U3vRegisterIo<T>>::open_stream(self, U3vStreamOptions::default())
            .map_err(|e| format!("Failed to open the USB stream: {e}"))?;
        self.acquisition_start()
            .map_err(|e| format!("Failed to start acquisition: {e}"))?;
        Ok(Frames::U3v(frames))
    }

    fn start_events(
        &mut self,
        names: &[String],
    ) -> Result<(CameraEvents, EventConfiguration), String> {
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let saved = self
            .configure_events_restorable(&names)
            .map_err(|error| viva_zenoh_api::setup_error_message(&error))?;
        match self.open_event_stream() {
            Ok(events) => Ok((CameraEvents::U3v(events), saved)),
            Err(source) => {
                let restoration = Camera::restore_events(self, saved);
                Err(viva_zenoh_api::setup_error_message(&EventSetupError {
                    source,
                    restoration,
                }))
            }
        }
    }

    fn restore_events(&mut self, saved: EventConfiguration) -> EventRestoreReport {
        Camera::restore_events(self, saved)
    }

    fn read_event(&mut self, event: &Event) -> ReadEvent {
        read_event_of(self, event)
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

/// The events of either kind of channel: a GigE Vision message channel or a
/// USB3 Vision event interface.
pub(crate) enum CameraEvents {
    Gige(Box<EventStream>),
    U3v(U3vEventStream),
}

impl CameraEvents {
    /// The next event; an error once the channel has failed.
    pub(crate) async fn next(&self) -> Result<Event, GenicamError> {
        match self {
            CameraEvents::Gige(events) => events.next().await,
            CameraEvents::U3v(events) => events.next().await,
        }
    }

    /// Whether events carry a stream channel and block: GigE Vision's do.
    pub(crate) fn carry_blocks(&self) -> bool {
        matches!(self, CameraEvents::Gige(_))
    }

    /// Stop receiving and wait until the channel has let go of the camera: a
    /// USB3 Vision reader is joined and EventEnable restored, and an error
    /// says it could not be.
    pub(crate) async fn shutdown(self) -> Result<(), String> {
        match self {
            CameraEvents::Gige(events) => {
                drop(events);
                Ok(())
            }
            CameraEvents::U3v(events) => events
                .shutdown()
                .await
                .map_err(|error| format!("The event channel was not closed cleanly: {error}")),
        }
    }
}
