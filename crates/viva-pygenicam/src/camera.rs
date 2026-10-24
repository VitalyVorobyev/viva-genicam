//! `PyCamera`: unified handle over GigE and U3V cameras.
//!
//! The Rust generic `Camera<T: RegisterIo>` is hidden behind an enum so Python
//! sees a single `Camera` class regardless of transport.
//!
//! A `NodeMap` is not `Sync`, so each camera sits behind a `Mutex`, and any
//! Python thread may use it: calls are serialised on the mutex. Every call
//! takes the mutex — and waits on the camera — with the GIL released, so
//! other threads run meanwhile. `close` empties the slot; the camera is then
//! dropped, which releases control privilege.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use if_addrs::{get_if_addrs, IfAddr};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList};

use viva_genapi::RegisterIo;
use viva_genicam::gige::{Iface, IfaceSelector};
use viva_genicam::{
    connect_gige_as, open_u3v_device, Access, Camera, ChunkConfig, ControlState, GenicamError,
    GigeRegisterIo, Roi, U3vRegisterIo,
};
use viva_u3v::{BulkIn, NusbTransfer, U3vDevice, U3vError, UsbSpeed, UsbTransfer};

use crate::discovery::{PyGigeDeviceInfo, PyU3vDeviceInfo};
use crate::errors::{genicam_error, gige_only, parse_error, to_py, transport_error, IntoPyErr};
use crate::events::{PyEvent, PyEventStream};
use crate::frame::{posix_secs, PyFrame};
use crate::nodemap::{
    collect_categories, collect_node_info, collect_node_names, collect_skipped, feature_state_dict,
    node_info_list, node_info_with_state, skipped_list,
};
use crate::runtime::runtime;
use crate::stream::{build_gige_stream, build_u3v_stream, GigeStreamRequest, PyFrameStream};

pub(crate) type GigeCamera = Camera<GigeRegisterIo>;
pub(crate) type U3vCamera = Camera<U3vRegisterIo<UsbLink>>;

/// A USB3 Vision camera's link: a USB device, or the in-process fake
/// (`viva_genicam.testing.FakeU3vCamera`), behind one type so Python sees
/// one `Camera` class.
pub(crate) enum UsbLink {
    Usb(Arc<NusbTransfer>),
    Fake(Arc<viva_fake_u3v::FakeU3vTransport>),
}

impl UsbTransfer for UsbLink {
    fn bulk_write(&self, endpoint: u8, data: &[u8], timeout: Duration) -> Result<usize, U3vError> {
        match self {
            UsbLink::Usb(usb) => usb.bulk_write(endpoint, data, timeout),
            UsbLink::Fake(fake) => fake.bulk_write(endpoint, data, timeout),
        }
    }

    fn bulk_read(
        &self,
        endpoint: u8,
        buf: &mut [u8],
        timeout: Duration,
    ) -> Result<usize, U3vError> {
        match self {
            UsbLink::Usb(usb) => usb.bulk_read(endpoint, buf, timeout),
            UsbLink::Fake(fake) => fake.bulk_read(endpoint, buf, timeout),
        }
    }

    // Each link's own queue: several transfers in flight on USB.
    fn bulk_in(self: &Arc<Self>, endpoint: u8) -> Result<Box<dyn BulkIn>, U3vError> {
        match self.as_ref() {
            UsbLink::Usb(usb) => usb.bulk_in(endpoint),
            UsbLink::Fake(fake) => fake.bulk_in(endpoint),
        }
    }

    fn max_packet_size(&self, endpoint: u8) -> Option<usize> {
        match self {
            UsbLink::Usb(usb) => usb.max_packet_size(endpoint),
            UsbLink::Fake(fake) => fake.max_packet_size(endpoint),
        }
    }

    fn link_speed(&self) -> Option<UsbSpeed> {
        match self {
            UsbLink::Usb(usb) => usb.link_speed(),
            UsbLink::Fake(fake) => fake.link_speed(),
        }
    }

    // How an unplug is told apart on macOS, whose transfers do not say.
    fn is_present(&self) -> Option<bool> {
        match self {
            UsbLink::Usb(usb) => usb.is_present(),
            UsbLink::Fake(fake) => fake.is_present(),
        }
    }
}

/// Open a USB3 Vision camera over `device`, reading its XML.
pub(crate) fn open_u3v(device: U3vDevice<UsbLink>) -> Result<PyCamera, GenicamError> {
    let (camera, xml) = open_u3v_device(device)?;
    Ok(PyCamera {
        inner: CameraInner::U3v {
            xml,
            camera: Arc::new(Mutex::new(Some(camera))),
        },
    })
}

/// Pick the NIC whose subnet contains `device_ip`. Returns `None` if no
/// interface matches; the caller should raise a user-actionable error.
pub(crate) fn auto_iface(device_ip: Ipv4Addr) -> Option<Iface> {
    if device_ip.is_loopback() {
        return Iface::from_ipv4(Ipv4Addr::LOCALHOST).ok();
    }
    let ifaces = get_if_addrs().ok()?;
    for iface in ifaces {
        let IfAddr::V4(v4) = iface.addr else { continue };
        let ip = u32::from(v4.ip);
        let mask = u32::from(v4.netmask);
        let target = u32::from(device_ip);
        if (ip & mask) == (target & mask) {
            if let Ok(resolved) = Iface::from_system(&iface.name) {
                return Some(resolved);
            }
        }
    }
    None
}

/// Lock a camera's slot, refusing a closed camera.
pub(crate) fn lock_open<T>(camera: &Mutex<Option<T>>) -> PyResult<MutexGuard<'_, Option<T>>> {
    let guard = camera
        .lock()
        .map_err(|_| genicam_error("camera mutex poisoned"))?;
    if guard.is_none() {
        return Err(genicam_error("the camera is closed"));
    }
    Ok(guard)
}

pub(crate) enum CameraInner {
    Gige {
        // Boxed: a GigE camera's description outweighs the rest of either
        // variant.
        device_info: Box<viva_genicam::gige::DeviceInfo>,
        iface: Mutex<Option<Iface>>,
        xml: String,
        camera: Arc<Mutex<Option<GigeCamera>>>,
    },
    U3v {
        xml: String,
        camera: Arc<Mutex<Option<U3vCamera>>>,
    },
}

#[pyclass(name = "Camera", module = "viva_genicam._native")]
pub(crate) struct PyCamera {
    pub(crate) inner: CameraInner,
}

/// Run `$body` with `$cam` bound to the open camera, whichever its transport,
/// with the GIL released. `$body` is a `PyResult`.
macro_rules! with_camera {
    ($self:ident, $py:ident, |$cam:ident| $body:expr) => {
        $py.detach(|| -> PyResult<_> {
            match &$self.inner {
                CameraInner::Gige { camera, .. } => {
                    let mut guard = lock_open(camera)?;
                    let $cam = guard
                        .as_mut()
                        .expect("lock_open checked the camera is open");
                    $body
                }
                CameraInner::U3v { camera, .. } => {
                    let mut guard = lock_open(camera)?;
                    let $cam = guard
                        .as_mut()
                        .expect("lock_open checked the camera is open");
                    $body
                }
            }
        })
    };
}

/// [`with_camera`] for what only a GigE Vision camera has; `$what` names it
/// in the error a USB3 Vision camera raises.
macro_rules! with_gige {
    ($self:ident, $py:ident, $what:expr, |$cam:ident| $body:expr) => {
        $py.detach(|| -> PyResult<_> {
            match &$self.inner {
                CameraInner::Gige { camera, .. } => {
                    let mut guard = lock_open(camera)?;
                    let $cam = guard
                        .as_mut()
                        .expect("lock_open checked the camera is open");
                    $body
                }
                CameraInner::U3v { .. } => Err(gige_only($what)),
            }
        })
    };
}

/// Resolve the `iface=` argument, however the caller spelled it.
///
/// An IPv4 address and an OS interface name are both accepted, matching
/// `viva-camctl --iface` and `viva-service --iface`. Before this the Python
/// surface took the name only, so the address a user had just read off
/// `discover()` was not a legal argument to `connect_gige()`
/// ([#109](https://github.com/VitalyVorobyev/viva-genicam/issues/109)).
pub(crate) fn iface_from_str(selector: &str) -> PyResult<Iface> {
    selector
        .parse::<IfaceSelector>()
        .expect("IfaceSelector parsing is infallible")
        .resolve()
        .map_err(|e| parse_error(format!("iface '{selector}': {e}")))
}

fn parse_ipv4(what: &str, text: &str) -> PyResult<Ipv4Addr> {
    text.parse()
        .map_err(|err| parse_error(format!("{what} '{text}': {err}")))
}

fn control_state_str(state: ControlState) -> &'static str {
    match state {
        ControlState::Held => "Held",
        ControlState::Monitor => "Monitor",
        ControlState::Lost => "Lost",
        ControlState::Released => "Released",
        _ => "Unknown",
    }
}

/// Features read from a frame's chunks or an event's data: each name with
/// its value, or the exception that kept it from being read.
fn feature_values(
    py: Python<'_>,
    values: Vec<(String, Result<String, GenicamError>)>,
) -> PyResult<Bound<'_, PyDict>> {
    let dict = PyDict::new(py);
    for (name, value) in values {
        match value {
            Ok(text) => dict.set_item(name, text)?,
            Err(err) => dict.set_item(name, to_py(err).into_value(py))?,
        }
    }
    Ok(dict)
}

#[pyfunction]
#[pyo3(signature = (device_info, iface=None, monitor=false, exclusive=false))]
fn connect_gige(
    py: Python<'_>,
    device_info: PyGigeDeviceInfo,
    iface: Option<&str>,
    monitor: bool,
    exclusive: bool,
) -> PyResult<PyCamera> {
    let access = match (monitor, exclusive) {
        (true, true) => {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "monitor=True claims nothing and exclusive=True claims exclusive access: \
                 pass one of them",
            ));
        }
        (true, false) => Access::Monitor,
        (false, true) => Access::Exclusive,
        (false, false) => Access::Control,
    };
    let iface_resolved = match iface {
        Some(name) => Some(iface_from_str(name)?),
        None => None,
    };
    let info = device_info.inner.clone();
    let (camera, xml) = py
        .detach(|| runtime().block_on(async move { connect_gige_as(&info, access).await }))
        .into_py_err()?;
    Ok(PyCamera {
        inner: CameraInner::Gige {
            device_info: Box::new(device_info.inner),
            iface: Mutex::new(iface_resolved),
            xml,
            camera: Arc::new(Mutex::new(Some(camera))),
        },
    })
}

#[pyfunction]
fn connect_u3v(py: Python<'_>, device_info: PyU3vDeviceInfo) -> PyResult<PyCamera> {
    let info = device_info.inner;
    py.detach(|| {
        let device = NusbTransfer::open(&info)
            .and_then(|usb| {
                let iface = usb.interfaces().clone();
                U3vDevice::open(
                    Arc::new(UsbLink::Usb(Arc::new(usb))),
                    iface.control_ep_in,
                    iface.control_ep_out,
                    iface.stream_ep_in,
                    iface.event_ep_in,
                )
            })
            .map_err(|source| GenicamError::U3v {
                during: "open the device",
                source,
            })?;
        open_u3v(device)
    })
    .into_py_err()
}

impl PyCamera {
    /// The interface a GigE stream or event channel receives on: the one
    /// asked for, else the one given at connect, else the one whose subnet
    /// holds the camera.
    fn receive_iface(&self, asked: Option<&str>) -> PyResult<Iface> {
        let CameraInner::Gige {
            device_info,
            iface: iface_cell,
            ..
        } = &self.inner
        else {
            return Err(gige_only("choosing a receive interface"));
        };
        let resolved = match asked {
            Some(name) => Some(iface_from_str(name)?),
            None => {
                let stored = iface_cell
                    .lock()
                    .map_err(|_| parse_error("iface mutex poisoned"))?
                    .clone();
                stored.or_else(|| auto_iface(device_info.ip))
            }
        };
        resolved.ok_or_else(|| {
            parse_error(format!(
                "no network interface matched camera IP {}; pass iface=... \
                 to connect_gige(...) or to this call",
                device_info.ip
            ))
        })
    }
}

#[pymethods]
impl PyCamera {
    /// Transport kind: "gige" or "u3v".
    #[getter]
    fn transport(&self) -> &'static str {
        match &self.inner {
            CameraInner::Gige { .. } => "gige",
            CameraInner::U3v { .. } => "u3v",
        }
    }

    /// Raw GenICam XML fetched from the camera.
    #[getter]
    fn xml(&self) -> &str {
        match &self.inner {
            CameraInner::Gige { xml, .. } => xml,
            CameraInner::U3v { xml, .. } => xml,
        }
    }

    /// Whether [`close`](Self::close) was called.
    #[getter]
    fn closed(&self, py: Python<'_>) -> PyResult<bool> {
        // Another thread may hold the camera for a while: wait without the GIL.
        let closed = py.detach(|| match &self.inner {
            CameraInner::Gige { camera, .. } => camera.lock().ok().map(|c| c.is_none()),
            CameraInner::U3v { camera, .. } => camera.lock().ok().map(|c| c.is_none()),
        });
        closed.ok_or_else(|| genicam_error("camera mutex poisoned"))
    }

    /// Close the camera: a GigE camera gives control privilege back. Every
    /// later call raises; closing again does nothing.
    fn close(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| {
            // Taken under the lock, dropped after it: the drop waits for the
            // camera to acknowledge the release.
            match &self.inner {
                CameraInner::Gige { camera, .. } => {
                    let taken = camera
                        .lock()
                        .map_err(|_| genicam_error("camera mutex poisoned"))?
                        .take();
                    drop(taken);
                }
                CameraInner::U3v { camera, .. } => {
                    let taken = camera
                        .lock()
                        .map_err(|_| genicam_error("camera mutex poisoned"))?
                        .take();
                    drop(taken);
                }
            }
            Ok(())
        })
    }

    // ── features as text ────────────────────────────────────────────────

    fn get(&self, py: Python<'_>, name: &str) -> PyResult<String> {
        with_camera!(self, py, |cam| cam.get(name).into_py_err())
    }

    fn set(&self, py: Python<'_>, name: &str, value: &str) -> PyResult<()> {
        with_camera!(self, py, |cam| cam.set(name, value).into_py_err())
    }

    // ── features by type ────────────────────────────────────────────────

    fn get_integer(&self, py: Python<'_>, name: &str) -> PyResult<i64> {
        with_camera!(self, py, |cam| cam.get_integer(name).into_py_err())
    }

    fn set_integer(&self, py: Python<'_>, name: &str, value: i64) -> PyResult<()> {
        with_camera!(self, py, |cam| cam.set_integer(name, value).into_py_err())
    }

    fn get_float(&self, py: Python<'_>, name: &str) -> PyResult<f64> {
        with_camera!(self, py, |cam| cam.get_float(name).into_py_err())
    }

    fn set_float(&self, py: Python<'_>, name: &str, value: f64) -> PyResult<()> {
        with_camera!(self, py, |cam| cam.set_float(name, value).into_py_err())
    }

    fn get_bool(&self, py: Python<'_>, name: &str) -> PyResult<bool> {
        with_camera!(self, py, |cam| cam.get_bool(name).into_py_err())
    }

    fn set_bool(&self, py: Python<'_>, name: &str, value: bool) -> PyResult<()> {
        with_camera!(self, py, |cam| cam.set_bool(name, value).into_py_err())
    }

    fn get_enum(&self, py: Python<'_>, name: &str) -> PyResult<String> {
        with_camera!(self, py, |cam| cam.get_enum(name).into_py_err())
    }

    fn set_enum(&self, py: Python<'_>, name: &str, entry: &str) -> PyResult<()> {
        with_camera!(self, py, |cam| cam.set_enum(name, entry).into_py_err())
    }

    fn get_string(&self, py: Python<'_>, name: &str) -> PyResult<String> {
        with_camera!(self, py, |cam| cam.get_string(name).into_py_err())
    }

    fn set_string(&self, py: Python<'_>, name: &str, value: &str) -> PyResult<()> {
        with_camera!(self, py, |cam| cam.set_string(name, value).into_py_err())
    }

    fn get_register<'py>(&self, py: Python<'py>, name: &str) -> PyResult<Bound<'py, PyBytes>> {
        let data = with_camera!(self, py, |cam| cam.get_register(name).into_py_err())?;
        Ok(PyBytes::new(py, &data))
    }

    fn set_register(&self, py: Python<'_>, name: &str, data: &[u8]) -> PyResult<()> {
        with_camera!(self, py, |cam| cam.set_register(name, data).into_py_err())
    }

    /// Everything a viewer needs about a feature at once, as a dict.
    fn feature_state<'py>(&self, py: Python<'py>, name: &str) -> PyResult<Bound<'py, PyDict>> {
        let state = with_camera!(self, py, |cam| cam.feature_state(name).into_py_err())?;
        feature_state_dict(py, state)
    }

    fn enum_entries(&self, py: Python<'_>, name: &str) -> PyResult<Vec<String>> {
        with_camera!(self, py, |cam| cam.enum_entries(name).into_py_err())
    }

    /// The entries the enumeration accepts now, after each entry's own
    /// `pIsImplemented` and `pIsAvailable`.
    fn available_enum_entries(&self, py: Python<'_>, name: &str) -> PyResult<Vec<String>> {
        with_camera!(self, py, |cam| cam
            .nodemap()
            .available_enum_entries(name, cam.transport())
            .map_err(GenicamError::from)
            .into_py_err())
    }

    // ── refresh ─────────────────────────────────────────────────────────

    fn generation(&self, py: Python<'_>) -> PyResult<u64> {
        with_camera!(self, py, |cam| Ok(cam.generation()))
    }

    fn changed_since(&self, py: Python<'_>, since: u64) -> PyResult<(Vec<String>, u64)> {
        with_camera!(self, py, |cam| Ok(cam.changed_since(since)))
    }

    fn poll(&self, py: Python<'_>) -> PyResult<Vec<String>> {
        with_camera!(self, py, |cam| Ok(cam.poll()))
    }

    /// Forget `name`'s cached value, and those of the registers it is read
    /// through and of what depends on it (`Camera::invalidate`).
    fn invalidate(&self, py: Python<'_>, name: &str) -> PyResult<()> {
        with_camera!(self, py, |cam| {
            cam.invalidate(name);
            Ok(())
        })
    }

    /// Forget every cached value (`Camera::invalidate_all`).
    fn invalidate_all(&self, py: Python<'_>) -> PyResult<()> {
        with_camera!(self, py, |cam| {
            cam.invalidate_all();
            Ok(())
        })
    }

    // ── standard features ───────────────────────────────────────────────

    fn exposure_time(&self, py: Python<'_>) -> PyResult<f64> {
        with_camera!(self, py, |cam| cam.exposure_time().into_py_err())
    }

    /// Write the exposure time through whichever feature the camera has for it —
    /// microseconds for `ExposureTime`/`ExposureTimeAbs`, device units for `ExposureTimeRaw` — and return that feature's name.
    fn set_exposure_time(&self, py: Python<'_>, value: f64) -> PyResult<&'static str> {
        with_camera!(self, py, |cam| cam
            .set_exposure_time(value)
            .map(|f| f.name)
            .into_py_err())
    }

    fn gain(&self, py: Python<'_>) -> PyResult<f64> {
        with_camera!(self, py, |cam| cam.gain().into_py_err())
    }

    /// Write the gain through whichever feature the camera has for it —
    /// dB for `Gain`, device units for `GainRaw` — and return that feature's name.
    fn set_gain(&self, py: Python<'_>, value: f64) -> PyResult<&'static str> {
        with_camera!(self, py, |cam| cam
            .set_gain(value)
            .map(|f| f.name)
            .into_py_err())
    }

    fn gain_selectors(&self, py: Python<'_>) -> PyResult<Vec<String>> {
        with_camera!(self, py, |cam| cam.gain_selectors().into_py_err())
    }

    fn select_gain(&self, py: Python<'_>, selector: &str) -> PyResult<()> {
        with_camera!(self, py, |cam| cam.select_gain(selector).into_py_err())
    }

    fn acquisition_mode(&self, py: Python<'_>) -> PyResult<String> {
        with_camera!(self, py, |cam| cam.acquisition_mode().into_py_err())
    }

    fn set_acquisition_mode(&self, py: Python<'_>, mode: &str) -> PyResult<()> {
        with_camera!(self, py, |cam| cam.set_acquisition_mode(mode).into_py_err())
    }

    fn enable_trigger(&self, py: Python<'_>, selector: &str, source: &str) -> PyResult<()> {
        with_camera!(self, py, |cam| cam
            .enable_trigger(selector, source)
            .into_py_err())
    }

    fn disable_trigger(&self, py: Python<'_>, selector: &str) -> PyResult<()> {
        with_camera!(self, py, |cam| cam.disable_trigger(selector).into_py_err())
    }

    fn trigger_software(&self, py: Python<'_>) -> PyResult<()> {
        with_camera!(self, py, |cam| cam.trigger_software().into_py_err())
    }

    /// The region of interest as `(x, y, width, height)`.
    fn roi(&self, py: Python<'_>) -> PyResult<(i64, i64, i64, i64)> {
        with_camera!(self, py, |cam| cam
            .roi()
            .map(|roi| (roi.x, roi.y, roi.width, roi.height))
            .into_py_err())
    }

    fn set_roi(&self, py: Python<'_>, x: i64, y: i64, width: i64, height: i64) -> PyResult<()> {
        let roi = Roi {
            x,
            y,
            width,
            height,
        };
        with_camera!(self, py, |cam| cam.set_roi(roi).into_py_err())
    }

    /// The binning as `(horizontal, vertical)`.
    fn binning(&self, py: Python<'_>) -> PyResult<(i64, i64)> {
        with_camera!(self, py, |cam| cam.binning().into_py_err())
    }

    fn set_binning(&self, py: Python<'_>, horizontal: i64, vertical: i64) -> PyResult<()> {
        with_camera!(self, py, |cam| cam
            .set_binning(horizontal, vertical)
            .into_py_err())
    }

    fn frame_rate(&self, py: Python<'_>) -> PyResult<f64> {
        with_camera!(self, py, |cam| cam.frame_rate().into_py_err())
    }

    fn set_frame_rate(&self, py: Python<'_>, hz: f64) -> PyResult<&'static str> {
        with_camera!(self, py, |cam| cam
            .set_frame_rate(hz)
            .map(|f| f.name)
            .into_py_err())
    }

    fn disable_frame_rate(&self, py: Python<'_>) -> PyResult<()> {
        with_camera!(self, py, |cam| cam.disable_frame_rate().into_py_err())
    }

    fn user_sets(&self, py: Python<'_>) -> PyResult<Vec<String>> {
        with_camera!(self, py, |cam| cam.user_sets().into_py_err())
    }

    fn load_user_set(&self, py: Python<'_>, user_set: &str) -> PyResult<()> {
        with_camera!(self, py, |cam| cam.load_user_set(user_set).into_py_err())
    }

    fn save_user_set(&self, py: Python<'_>, user_set: &str) -> PyResult<()> {
        with_camera!(self, py, |cam| cam.save_user_set(user_set).into_py_err())
    }

    fn set_default_user_set(&self, py: Python<'_>, user_set: &str) -> PyResult<&'static str> {
        with_camera!(self, py, |cam| cam
            .set_default_user_set(user_set)
            .map(|f| f.name)
            .into_py_err())
    }

    // ── commands ────────────────────────────────────────────────────────

    /// Execute a `<Command>` node, and by default wait until the camera
    /// reports it finished.
    ///
    /// Exposed because there was no verb for it (issue #121): `set(name, "1")`
    /// already dispatched commands and ignored the value, which is neither
    /// discoverable nor documented. Named `execute` to match
    /// `Camera::execute_command` in the Rust API and the `execute` key the
    /// service and Studio already use. Waiting is `Camera::execute_and_wait`:
    /// GenICam asks applications to poll `IsDone` after every execute.
    #[pyo3(signature = (name, wait=true, timeout_s=5.0))]
    fn execute(&self, py: Python<'_>, name: &str, wait: bool, timeout_s: f64) -> PyResult<bool> {
        let timeout = Duration::try_from_secs_f64(timeout_s)
            .map_err(|_| parse_error("timeout must be a non-negative number of seconds"))?;
        with_camera!(self, py, |cam| {
            if wait {
                match cam.execute_and_wait_blocking(name, timeout) {
                    Ok(()) => Ok(true),
                    Err(GenicamError::CommandNotDone { .. }) => Ok(false),
                    Err(err) => Err(err).into_py_err(),
                }
            } else {
                cam.execute_command(name).into_py_err().map(|()| false)
            }
        })
    }

    /// Whether the command `name` has finished: GenICam's `IsDone`.
    fn is_done(&self, py: Python<'_>, name: &str) -> PyResult<bool> {
        with_camera!(self, py, |cam| cam.is_done(name).into_py_err())
    }

    // ── configuration files ─────────────────────────────────────────────

    /// Save the streamable features to `path` (ADR-0028): a dict with
    /// `values`, the number saved, and `left_out`, `(feature, reason)` pairs.
    fn save_features<'py>(&self, py: Python<'py>, path: PathBuf) -> PyResult<Bound<'py, PyDict>> {
        let saved = with_camera!(self, py, |cam| {
            let file = std::fs::File::create(&path)
                .map_err(|e| parse_error(format!("create {}: {e}", path.display())))?;
            let mut out = std::io::BufWriter::new(file);
            cam.save_features(&mut out).into_py_err()
        })?;
        let dict = PyDict::new(py);
        dict.set_item("values", saved.values.len())?;
        let left_out: Vec<(String, String)> = saved
            .omitted
            .iter()
            .map(|o| (o.feature.clone(), o.reason.clone()))
            .collect();
        dict.set_item("left_out", left_out)?;
        Ok(dict)
    }

    /// Restore a configuration file: a dict with `written`,
    /// `written_on_retry`, `failures` and `warnings` (lists of strings).
    fn load_features<'py>(&self, py: Python<'py>, path: PathBuf) -> PyResult<Bound<'py, PyDict>> {
        let report = with_camera!(self, py, |cam| {
            let file = std::fs::File::open(&path)
                .map_err(|e| parse_error(format!("open {}: {e}", path.display())))?;
            let mut input = std::io::BufReader::new(file);
            cam.load_features(&mut input).into_py_err()
        })?;
        let dict = PyDict::new(py);
        dict.set_item("written", report.written)?;
        dict.set_item("written_on_retry", report.written_on_retry)?;
        let failures: Vec<String> = report.failures.iter().map(ToString::to_string).collect();
        dict.set_item("failures", failures)?;
        dict.set_item("warnings", report.warnings.clone())?;
        Ok(dict)
    }

    // ── acquisition ─────────────────────────────────────────────────────

    fn acquisition_start(&self, py: Python<'_>) -> PyResult<()> {
        with_camera!(self, py, |cam| cam.acquisition_start().into_py_err())
    }

    fn acquisition_stop(&self, py: Python<'_>) -> PyResult<()> {
        with_camera!(self, py, |cam| cam.acquisition_stop().into_py_err())
    }

    // ── time ────────────────────────────────────────────────────────────

    /// Fit the device-to-host clock mapping from `samples` timestamp latches
    /// `interval_ms` apart. A GigE stream opened afterwards gives each frame a
    /// `ts_host`; before, it has none (backlog DOC-11).
    #[pyo3(signature = (samples=8, interval_ms=20))]
    fn time_calibrate(&self, py: Python<'_>, samples: usize, interval_ms: u64) -> PyResult<()> {
        with_gige!(self, py, "time_calibrate", |cam| runtime()
            .block_on(cam.time_calibrate(samples, interval_ms))
            .into_py_err())
    }

    /// Reset the camera's timestamp counter, where it has a command for it,
    /// and forget the clock fit.
    fn time_reset(&self, py: Python<'_>) -> PyResult<()> {
        with_camera!(self, py, |cam| cam.time_reset().into_py_err())
    }

    /// Device ticks mapped to host time, as POSIX seconds; `None` before
    /// `time_calibrate`.
    fn map_dev_ts(&self, py: Python<'_>, ticks: u64) -> PyResult<Option<f64>> {
        with_camera!(self, py, |cam| Ok(cam
            .map_dev_ts(ticks)
            .and_then(posix_secs)))
    }

    // ── chunks and events ───────────────────────────────────────────────

    /// Turn chunk mode on (or off) and enable the chunks `selectors` name.
    #[pyo3(signature = (selectors, active=true))]
    fn configure_chunks(
        &self,
        py: Python<'_>,
        selectors: Vec<String>,
        active: bool,
    ) -> PyResult<()> {
        let config = ChunkConfig { selectors, active };
        with_camera!(self, py, |cam| cam.configure_chunks(&config).into_py_err())
    }

    /// Every feature `frame`'s chunks feed: name to value, or to the
    /// exception that kept it from being read.
    fn chunk_features<'py>(
        &self,
        py: Python<'py>,
        frame: PyRef<'py, PyFrame>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let frame = frame.inner.clone();
        let values = with_camera!(self, py, |cam| Ok(cam.chunk_features(&frame)))?;
        feature_values(py, values)
    }

    /// Every feature `event` feeds: name to value, or to the exception that
    /// kept it from being read.
    fn event_features<'py>(
        &self,
        py: Python<'py>,
        event: PyRef<'py, PyEvent>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let event = event.inner.clone();
        let values = with_camera!(self, py, |cam| Ok(cam.event_features(&event)))?;
        feature_values(py, values)
    }

    /// Enable the events `events` names and receive them: on GigE Vision,
    /// bind an event socket on `iface` (and `port`, 0 for any) and point the
    /// camera's message channel at it; on USB3 Vision, read the camera's
    /// event interface (`iface` and `port` do not apply).
    #[pyo3(signature = (events, iface=None, port=0))]
    fn open_event_stream(
        &self,
        py: Python<'_>,
        events: Vec<String>,
        iface: Option<&str>,
        port: u16,
    ) -> PyResult<PyEventStream> {
        if let CameraInner::U3v { camera, .. } = &self.inner {
            let ids: Vec<&str> = events.iter().map(String::as_str).collect();
            let stream = py.detach(|| {
                let mut guard = lock_open(camera)?;
                let cam = guard
                    .as_mut()
                    .expect("lock_open checked the camera is open");
                cam.configure_events(&ids)
                    .and_then(|()| cam.open_event_stream())
                    .into_py_err()
            })?;
            return Ok(PyEventStream::u3v(stream));
        }
        let iface = self.receive_iface(iface)?;
        let local_ip = iface.ipv4().ok_or_else(|| {
            parse_error(format!("interface {} has no IPv4 address", iface.name()))
        })?;
        let ids: Vec<&str> = events.iter().map(String::as_str).collect();
        let (stream, bound) = with_gige!(self, py, "events", |cam| runtime()
            .block_on(async {
                let stream = cam.open_event_stream(local_ip, port).await?;
                let bound = stream.local_addr()?.port();
                cam.configure_events(local_ip, bound, &ids).await?;
                Ok::<_, GenicamError>((stream, bound))
            })
            .into_py_err())?;
        Ok(PyEventStream::new(stream, bound))
    }

    // ── registers ───────────────────────────────────────────────────────

    /// `length` bytes of the camera's register space at `address`, past
    /// GenApi.
    #[pyo3(signature = (address, length=4))]
    fn read_register<'py>(
        &self,
        py: Python<'py>,
        address: u64,
        length: usize,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let data = with_camera!(self, py, |cam| cam
            .transport()
            .read(address, length)
            .map_err(GenicamError::from)
            .into_py_err())?;
        Ok(PyBytes::new(py, &data))
    }

    /// Write `data` to the camera's register space at `address`, past
    /// GenApi. Every cached value is forgotten: the write may have changed
    /// any of them.
    fn write_register(&self, py: Python<'_>, address: u64, data: &[u8]) -> PyResult<()> {
        with_camera!(self, py, |cam| {
            let written = cam
                .transport()
                .write(address, data)
                .map_err(GenicamError::from)
                .into_py_err();
            cam.invalidate_all();
            written
        })
    }

    // ── control privilege (GigE Vision) ─────────────────────────────────

    /// Where this session stands with control privilege: `"Held"`,
    /// `"Monitor"`, `"Lost"` or `"Released"`.
    #[getter]
    fn control_state(&self, py: Python<'_>) -> PyResult<&'static str> {
        with_gige!(self, py, "control_state", |cam| Ok(control_state_str(
            cam.transport().control_state()
        )))
    }

    fn reclaim_control(&self, py: Python<'_>) -> PyResult<()> {
        with_gige!(self, py, "reclaim_control", |cam| cam
            .transport()
            .reclaim_control()
            .into_py_err())
    }

    fn release_control(&self, py: Python<'_>) -> PyResult<()> {
        with_gige!(self, py, "release_control", |cam| cam
            .transport()
            .release_control()
            .into_py_err())
    }

    /// Store an IP configuration the camera uses from its next power-up, and
    /// switch persistent IP on.
    #[pyo3(signature = (ip, subnet, gateway="0.0.0.0"))]
    fn set_persistent_ip(
        &self,
        py: Python<'_>,
        ip: &str,
        subnet: &str,
        gateway: &str,
    ) -> PyResult<()> {
        let ip = parse_ipv4("ip", ip)?;
        let subnet = parse_ipv4("subnet", subnet)?;
        let gateway = parse_ipv4("gateway", gateway)?;
        with_gige!(self, py, "set_persistent_ip", |cam| {
            let transport = cam.transport();
            if transport.control_state() != ControlState::Held {
                return Err(genicam_error(
                    "setting a persistent IP needs control privilege, which this session does not hold",
                ));
            }
            let mut guard = transport.lock_device().into_py_err()?;
            let device = &mut *guard;
            runtime()
                .block_on(async {
                    device.write_persistent_ip(ip, subnet, gateway).await?;
                    device.enable_persistent_ip().await
                })
                .map_err(|err| transport_error(format!("persistent IP: {err}")))
        })
    }

    // ── introspection ───────────────────────────────────────────────────

    fn nodes(&self, py: Python<'_>) -> PyResult<Vec<String>> {
        with_camera!(self, py, |cam| Ok(collect_node_names(cam.nodemap())))
    }

    fn node_info<'py>(&self, py: Python<'py>, name: &str) -> PyResult<Option<Bound<'py, PyDict>>> {
        let info = with_camera!(self, py, |cam| Ok(node_info_with_state(
            name,
            cam.nodemap(),
            cam.transport()
        )))?;
        info.map(|info| info.into_dict(py)).transpose()
    }

    fn all_node_info<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let infos = with_camera!(self, py, |cam| Ok(collect_node_info(cam.nodemap())))?;
        node_info_list(py, infos)
    }

    fn skipped<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let skipped = with_camera!(self, py, |cam| Ok(collect_skipped(cam.nodemap())))?;
        skipped_list(py, skipped)
    }

    fn categories<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let categories = with_camera!(self, py, |cam| Ok(collect_categories(cam.nodemap())))?;
        let dict = PyDict::new(py);
        for (category, children) in categories {
            dict.set_item(category, children)?;
        }
        Ok(dict)
    }

    // ── streaming ───────────────────────────────────────────────────────

    #[pyo3(signature = (iface=None, packet_size=None, auto_packet_size=false, multicast=None, destination_port=None, packet_delay_ns=None, resend=true, deliver_incomplete=false, monitor=false))]
    #[allow(clippy::too_many_arguments)]
    fn open_stream(
        &self,
        py: Python<'_>,
        iface: Option<&str>,
        packet_size: Option<u32>,
        auto_packet_size: bool,
        multicast: Option<&str>,
        destination_port: Option<u16>,
        packet_delay_ns: Option<u64>,
        resend: bool,
        deliver_incomplete: bool,
        monitor: bool,
    ) -> PyResult<PyFrameStream> {
        match &self.inner {
            CameraInner::Gige { camera, .. } => {
                let iface_resolved = self.receive_iface(iface)?;
                let multicast_addr = match multicast {
                    Some(s) => Some(parse_ipv4("multicast group", s)?),
                    None => None,
                };
                build_gige_stream(
                    py,
                    camera.clone(),
                    Some(iface_resolved),
                    GigeStreamRequest {
                        packet_size,
                        auto_packet_size,
                        multicast: multicast_addr,
                        destination_port,
                        packet_delay_ns,
                        resend,
                        deliver_incomplete,
                        monitor,
                    },
                )
            }
            // USB3 Vision has no read-only access: one application opens
            // the device, and a monitor stream would wait for an acquisition
            // nobody starts.
            CameraInner::U3v { .. } if monitor => Err(gige_only("a monitor stream")),
            CameraInner::U3v { camera, .. } => build_u3v_stream(py, camera.clone()),
        }
    }
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyCamera>()?;
    m.add_function(wrap_pyfunction!(connect_gige, m)?)?;
    m.add_function(wrap_pyfunction!(connect_u3v, m)?)?;
    Ok(())
}
