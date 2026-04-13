//! `PyCamera`: unified handle over GigE and U3V cameras.
//!
//! The Rust generic `Camera<T: RegisterIo>` is hidden behind an enum so Python
//! sees a single `Camera` class regardless of transport.

use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};

use if_addrs::{IfAddr, get_if_addrs};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use viva_genicam::gige::nic::Iface;
use viva_genicam::{
    Camera, GigeRegisterIo, U3vRegisterIo, connect_gige_with_xml, connect_u3v_with_xml,
};
use viva_u3v::usb::RusbTransfer;

use crate::discovery::{PyGigeDeviceInfo, PyU3vDeviceInfo};
use crate::errors::{IntoPyErr, parse_error};
use crate::nodemap::{collect_categories, collect_node_info, collect_node_names, to_node_info};
use crate::runtime::runtime;
use crate::stream::{PyFrameStream, build_gige_stream, build_u3v_stream};

pub(crate) type GigeCamera = Camera<GigeRegisterIo>;
pub(crate) type U3vCamera = Camera<U3vRegisterIo<RusbTransfer>>;

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

pub(crate) enum CameraInner {
    Gige {
        #[allow(dead_code)]
        device_info: viva_genicam::gige::DeviceInfo,
        iface: Mutex<Option<Iface>>,
        xml: String,
        camera: Arc<Mutex<GigeCamera>>,
    },
    U3v {
        #[allow(dead_code)]
        device_info: viva_u3v::discovery::U3vDeviceInfo,
        xml: String,
        camera: Arc<Mutex<U3vCamera>>,
    },
}

#[pyclass(module = "viva_genicam._native", unsendable)]
pub(crate) struct PyCamera {
    pub(crate) inner: CameraInner,
}

fn iface_from_str(name: &str) -> PyResult<Iface> {
    Iface::from_system(name).map_err(|e| parse_error(format!("iface '{name}': {e}")))
}

#[pyfunction]
#[pyo3(signature = (device_info, iface=None))]
fn connect_gige(
    py: Python<'_>,
    device_info: PyGigeDeviceInfo,
    iface: Option<&str>,
) -> PyResult<PyCamera> {
    let iface_resolved = match iface {
        Some(name) => Some(iface_from_str(name)?),
        None => None,
    };
    let info = device_info.inner.clone();
    let (camera, xml) = py
        .allow_threads(|| runtime().block_on(async move { connect_gige_with_xml(&info).await }))
        .into_py_err()?;
    Ok(PyCamera {
        inner: CameraInner::Gige {
            device_info: device_info.inner,
            iface: Mutex::new(iface_resolved),
            xml,
            camera: Arc::new(Mutex::new(camera)),
        },
    })
}

#[pyfunction]
fn connect_u3v(py: Python<'_>, device_info: PyU3vDeviceInfo) -> PyResult<PyCamera> {
    let info = device_info.inner.clone();
    let (camera, xml) = py
        .allow_threads(|| connect_u3v_with_xml(&info))
        .into_py_err()?;
    Ok(PyCamera {
        inner: CameraInner::U3v {
            device_info: device_info.inner,
            xml,
            camera: Arc::new(Mutex::new(camera)),
        },
    })
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

    fn get(&self, py: Python<'_>, name: &str) -> PyResult<String> {
        py.allow_threads(|| match &self.inner {
            CameraInner::Gige { camera, .. } => {
                let g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
                g.get(name).into_py_err()
            }
            CameraInner::U3v { camera, .. } => {
                let g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
                g.get(name).into_py_err()
            }
        })
    }

    fn set(&self, py: Python<'_>, name: &str, value: &str) -> PyResult<()> {
        py.allow_threads(|| match &self.inner {
            CameraInner::Gige { camera, .. } => {
                let mut g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
                g.set(name, value).into_py_err()
            }
            CameraInner::U3v { camera, .. } => {
                let mut g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
                g.set(name, value).into_py_err()
            }
        })
    }

    fn set_exposure_time_us(&self, py: Python<'_>, value: f64) -> PyResult<()> {
        py.allow_threads(|| match &self.inner {
            CameraInner::Gige { camera, .. } => {
                let mut g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
                g.set_exposure_time_us(value).into_py_err()
            }
            CameraInner::U3v { camera, .. } => {
                let mut g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
                g.set_exposure_time_us(value).into_py_err()
            }
        })
    }

    fn set_gain_db(&self, py: Python<'_>, value: f64) -> PyResult<()> {
        py.allow_threads(|| match &self.inner {
            CameraInner::Gige { camera, .. } => {
                let mut g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
                g.set_gain_db(value).into_py_err()
            }
            CameraInner::U3v { camera, .. } => {
                let mut g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
                g.set_gain_db(value).into_py_err()
            }
        })
    }

    fn acquisition_start(&self, py: Python<'_>) -> PyResult<()> {
        py.allow_threads(|| match &self.inner {
            CameraInner::Gige { camera, .. } => {
                let mut g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
                g.acquisition_start().into_py_err()
            }
            CameraInner::U3v { camera, .. } => {
                let mut g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
                g.acquisition_start().into_py_err()
            }
        })
    }

    fn acquisition_stop(&self, py: Python<'_>) -> PyResult<()> {
        py.allow_threads(|| match &self.inner {
            CameraInner::Gige { camera, .. } => {
                let mut g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
                g.acquisition_stop().into_py_err()
            }
            CameraInner::U3v { camera, .. } => {
                let mut g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
                g.acquisition_stop().into_py_err()
            }
        })
    }

    fn enum_entries(&self, name: &str) -> PyResult<Vec<String>> {
        match &self.inner {
            CameraInner::Gige { camera, .. } => {
                let g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
                g.enum_entries(name).into_py_err()
            }
            CameraInner::U3v { camera, .. } => {
                let g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
                g.enum_entries(name).into_py_err()
            }
        }
    }

    fn nodes(&self) -> PyResult<Vec<String>> {
        match &self.inner {
            CameraInner::Gige { camera, .. } => {
                let g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
                Ok(collect_node_names(g.nodemap()))
            }
            CameraInner::U3v { camera, .. } => {
                let g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
                Ok(collect_node_names(g.nodemap()))
            }
        }
    }

    fn node_info<'py>(
        &self,
        py: Python<'py>,
        name: &str,
    ) -> PyResult<Option<Bound<'py, PyDict>>> {
        match &self.inner {
            CameraInner::Gige { camera, .. } => {
                let g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
                let node = g.nodemap().node(name);
                node.map(|n| to_node_info(py, name, n)).transpose()
            }
            CameraInner::U3v { camera, .. } => {
                let g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
                let node = g.nodemap().node(name);
                node.map(|n| to_node_info(py, name, n)).transpose()
            }
        }
    }

    fn all_node_info<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        match &self.inner {
            CameraInner::Gige { camera, .. } => {
                let g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
                collect_node_info(py, g.nodemap())
            }
            CameraInner::U3v { camera, .. } => {
                let g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
                collect_node_info(py, g.nodemap())
            }
        }
    }

    fn categories<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        match &self.inner {
            CameraInner::Gige { camera, .. } => {
                let g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
                collect_categories(py, g.nodemap())
            }
            CameraInner::U3v { camera, .. } => {
                let g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
                collect_categories(py, g.nodemap())
            }
        }
    }

    #[pyo3(signature = (iface=None, auto_packet_size=None, multicast=None, destination_port=None))]
    fn open_stream(
        &self,
        py: Python<'_>,
        iface: Option<&str>,
        auto_packet_size: Option<bool>,
        multicast: Option<&str>,
        destination_port: Option<u16>,
    ) -> PyResult<PyFrameStream> {
        match &self.inner {
            CameraInner::Gige {
                device_info,
                iface: iface_cell,
                camera,
                ..
            } => {
                let iface_resolved = match iface {
                    Some(name) => Some(iface_from_str(name)?),
                    None => {
                        let stored = iface_cell
                            .lock()
                            .map_err(|_| parse_error("iface mutex poisoned"))?
                            .clone();
                        stored.or_else(|| auto_iface(device_info.ip))
                    }
                };
                let iface_resolved = iface_resolved.ok_or_else(|| {
                    parse_error(format!(
                        "no network interface matched camera IP {}; pass iface=... \
                         to connect_gige(...) or camera.stream(iface=...)",
                        device_info.ip
                    ))
                })?;
                let multicast_addr = match multicast {
                    Some(s) => Some(
                        s.parse::<std::net::Ipv4Addr>()
                            .map_err(|e| parse_error(format!("invalid multicast ip: {e}")))?,
                    ),
                    None => None,
                };
                build_gige_stream(
                    py,
                    camera.clone(),
                    Some(iface_resolved),
                    auto_packet_size,
                    multicast_addr,
                    destination_port,
                )
            }
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
