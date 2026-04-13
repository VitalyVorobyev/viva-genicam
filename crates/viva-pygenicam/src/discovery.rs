//! Discovery bindings: GigE Vision (async UDP) and USB3 Vision (libusb).

use std::time::Duration;

use pyo3::prelude::*;
use viva_genicam::gige;

use crate::errors::transport_error;
use crate::runtime::runtime;

#[pyclass(frozen, module = "viva_genicam._native")]
#[derive(Clone)]
pub(crate) struct PyGigeDeviceInfo {
    #[pyo3(get)]
    ip: String,
    #[pyo3(get)]
    mac: String,
    #[pyo3(get)]
    manufacturer: Option<String>,
    #[pyo3(get)]
    model: Option<String>,
    pub(crate) inner: gige::DeviceInfo,
}

impl From<gige::DeviceInfo> for PyGigeDeviceInfo {
    fn from(info: gige::DeviceInfo) -> Self {
        let ip = info.ip.to_string();
        let mac = info
            .mac
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":");
        Self {
            ip,
            mac,
            manufacturer: info.manufacturer.clone(),
            model: info.model.clone(),
            inner: info,
        }
    }
}

#[pymethods]
impl PyGigeDeviceInfo {
    fn __repr__(&self) -> String {
        format!(
            "GigeDeviceInfo(ip={:?}, mac={:?}, manufacturer={:?}, model={:?})",
            self.ip, self.mac, self.manufacturer, self.model
        )
    }
}

#[pyclass(frozen, module = "viva_genicam._native")]
#[derive(Clone)]
pub(crate) struct PyU3vDeviceInfo {
    #[pyo3(get)]
    bus: u8,
    #[pyo3(get)]
    address: u8,
    #[pyo3(get)]
    vendor_id: u16,
    #[pyo3(get)]
    product_id: u16,
    #[pyo3(get)]
    serial: Option<String>,
    #[pyo3(get)]
    manufacturer: Option<String>,
    #[pyo3(get)]
    model: Option<String>,
    pub(crate) inner: viva_u3v::discovery::U3vDeviceInfo,
}

impl From<viva_u3v::discovery::U3vDeviceInfo> for PyU3vDeviceInfo {
    fn from(info: viva_u3v::discovery::U3vDeviceInfo) -> Self {
        Self {
            bus: info.bus,
            address: info.address,
            vendor_id: info.vendor_id,
            product_id: info.product_id,
            serial: info.serial.clone(),
            manufacturer: info.manufacturer.clone(),
            model: info.model.clone(),
            inner: info,
        }
    }
}

#[pymethods]
impl PyU3vDeviceInfo {
    fn __repr__(&self) -> String {
        format!(
            "U3vDeviceInfo(bus={}, address={}, vendor_id=0x{:04x}, product_id=0x{:04x}, \
             serial={:?}, manufacturer={:?}, model={:?})",
            self.bus,
            self.address,
            self.vendor_id,
            self.product_id,
            self.serial,
            self.manufacturer,
            self.model,
        )
    }
}

#[pyfunction]
#[pyo3(signature = (timeout_ms=500, iface=None, all=false))]
fn discover_gige(
    py: Python<'_>,
    timeout_ms: u64,
    iface: Option<&str>,
    all: bool,
) -> PyResult<Vec<PyGigeDeviceInfo>> {
    let timeout = Duration::from_millis(timeout_ms);
    let devices = py.allow_threads(|| {
        runtime().block_on(async move {
            match (iface, all) {
                (Some(name), _) => gige::discover_on_interface(timeout, name).await,
                (None, true) => gige::discover_all(timeout).await,
                (None, false) => gige::discover(timeout).await,
            }
        })
    });
    let devices = devices.map_err(|e| transport_error(e.to_string()))?;
    Ok(devices.into_iter().map(PyGigeDeviceInfo::from).collect())
}

#[pyfunction]
fn discover_u3v(py: Python<'_>) -> PyResult<Vec<PyU3vDeviceInfo>> {
    let devices =
        py.allow_threads(viva_u3v::discovery::discover).map_err(|e| transport_error(e.to_string()))?;
    Ok(devices.into_iter().map(PyU3vDeviceInfo::from).collect())
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyGigeDeviceInfo>()?;
    m.add_class::<PyU3vDeviceInfo>()?;
    m.add_function(wrap_pyfunction!(discover_gige, m)?)?;
    m.add_function(wrap_pyfunction!(discover_u3v, m)?)?;
    Ok(())
}
