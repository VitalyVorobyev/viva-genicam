//! GigE Vision cameras for programs without an async runtime.
//!
//! Discovery and opening a camera are asynchronous in the rest of this
//! crate; here they block. A camera opened with [`connect`] carries a small
//! runtime of its own — one worker thread that drives its socket, its
//! control-channel keepalive and its timers — and shuts it down when it is
//! dropped, after giving control back. Everything else is the ordinary
//! [`Camera`]: its feature access, the standard-feature helpers and
//! [`Camera::open_stream`] are synchronous already, and frames come from
//! [`FrameStream::next_frame_blocking`](crate::FrameStream::next_frame_blocking).
//! Commands wait with [`Camera::execute_and_wait_blocking`].
//!
//! ```rust,no_run
//! use std::time::Duration;
//! use viva_genicam::{blocking, StreamOptions};
//!
//! # fn main() -> Result<(), viva_genicam::GenicamError> {
//! let devices = blocking::discover(Duration::from_millis(500))?;
//! let mut camera = blocking::connect(&devices[0])?;
//! camera.set_exposure_time(5_000.0)?;
//! let mut frames = camera.open_stream(StreamOptions::new())?;
//! camera.acquisition_start()?;
//! let frame = frames.next_frame_blocking(Duration::from_secs(1))?;
//! camera.acquisition_stop()?;
//! # Ok(())
//! # }
//! ```
//!
//! These functions refuse to run inside an async runtime, where blocking
//! would stall it: there, use [`gige::discover`](crate::gige::discover) and
//! [`connect_gige`](crate::connect_gige).
//!
//! USB3 Vision needs nothing from here to open a camera — `connect_u3v` is
//! synchronous — but its frame stream still runs on a Tokio runtime.

use std::net::Ipv4Addr;
use std::time::Duration;

use crate::gige::{DeviceInfo, DiscoveryOptions};
use crate::{Access, Camera, GenicamError, GigeRegisterIo, camera_runtime, connect_here};

/// Refuse to block inside an async runtime, which would stall it — and which
/// Tokio refuses with a panic.
fn outside_a_runtime(what: &'static str) -> Result<(), GenicamError> {
    match tokio::runtime::Handle::try_current() {
        Ok(_) => Err(GenicamError::InsideRuntime(what)),
        Err(_) => Ok(()),
    }
}

/// Run `fut` to completion on a short-lived runtime.
fn run<F: std::future::Future>(what: &'static str, fut: F) -> Result<F::Output, GenicamError> {
    outside_a_runtime(what)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(crate::error::network("start a runtime"))?;
    Ok(runtime.block_on(fut))
}

/// Find GigE Vision cameras by broadcast: [`gige::discover`](crate::gige::discover),
/// blocking for `timeout`.
pub fn discover(timeout: Duration) -> Result<Vec<DeviceInfo>, GenicamError> {
    discover_with(timeout, &DiscoveryOptions::new())
}

/// Find GigE Vision cameras as `options` say:
/// [`gige::discover_with`](crate::gige::discover_with), blocking for
/// `timeout`.
pub fn discover_with(
    timeout: Duration,
    options: &DiscoveryOptions,
) -> Result<Vec<DeviceInfo>, GenicamError> {
    run(
        "discover_with",
        crate::gige::discover_with(timeout, options),
    )?
    .map_err(crate::error::gige("discovery"))
}

/// Ask the camera at `ip` directly: [`gige::discover_ip`](crate::gige::discover_ip),
/// blocking for up to `timeout`.
pub fn discover_ip(ip: Ipv4Addr, timeout: Duration) -> Result<Option<DeviceInfo>, GenicamError> {
    run("discover_ip", crate::gige::discover_ip(ip, timeout))?
        .map_err(crate::error::gige("discovery"))
}

/// Open a GigE Vision camera and take control of it:
/// [`connect_gige`](crate::connect_gige), blocking.
pub fn connect(device: &DeviceInfo) -> Result<Camera<GigeRegisterIo>, GenicamError> {
    connect_as(device, Access::Control).map(|(camera, _xml)| camera)
}

/// Open a GigE Vision camera with the given [`Access`], returning the
/// GenICam XML it was built from too: [`connect_gige_as`](crate::connect_gige_as),
/// blocking.
pub fn connect_as(
    device: &DeviceInfo,
    access: Access,
) -> Result<(Camera<GigeRegisterIo>, String), GenicamError> {
    outside_a_runtime("connect_as")?;
    let runtime = camera_runtime()?;
    let (mut camera, xml) = runtime.block_on(connect_here(device, access))?;
    camera.transport_mut().adopt_runtime(runtime);
    Ok((camera, xml))
}
