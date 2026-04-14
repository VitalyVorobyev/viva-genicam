//! In-process fake GigE Vision camera — Python binding over `viva-fake-gige`.
//!
//! Lets users exercise the full bindings (discovery, control, streaming)
//! without any hardware and without shelling out to a binary, so
//! `pip install viva-genicam` is self-sufficient.

use std::net::Ipv4Addr;
use std::time::Duration;

use pyo3::prelude::*;
use viva_fake_gige::{FakeCamera, MONO8, RGB8};
use viva_genicam::gige;

use crate::discovery::PyGigeDeviceInfo;
use crate::errors::{parse_error, transport_error};
use crate::runtime::runtime;

fn pixel_format_code(name: &str) -> PyResult<u32> {
    match name {
        "Mono8" | "mono8" => Ok(MONO8),
        "RGB8" | "rgb8" | "RGB8Packed" => Ok(RGB8),
        other => Err(parse_error(format!(
            "unsupported pixel_format '{other}'; expected 'Mono8' or 'RGB8'"
        ))),
    }
}

#[pyclass(name = "FakeGigeCamera", module = "viva_genicam._native.testing", unsendable)]
pub(crate) struct PyFakeGigeCamera {
    width: u32,
    height: u32,
    fps: u32,
    bind_ip: Ipv4Addr,
    port: u16,
    pixel_format: u32,
    handle: Option<FakeCamera>,
}

#[pymethods]
impl PyFakeGigeCamera {
    #[new]
    #[pyo3(signature = (
        width = 640,
        height = 480,
        fps = 30,
        bind_ip = "127.0.0.1",
        port = 3956,
        pixel_format = "Mono8",
    ))]
    fn new(
        width: u32,
        height: u32,
        fps: u32,
        bind_ip: &str,
        port: u16,
        pixel_format: &str,
    ) -> PyResult<Self> {
        let ip: Ipv4Addr = bind_ip
            .parse()
            .map_err(|e| parse_error(format!("invalid bind_ip '{bind_ip}': {e}")))?;
        let code = pixel_format_code(pixel_format)?;
        Ok(Self {
            width,
            height,
            fps,
            bind_ip: ip,
            port,
            pixel_format: code,
            handle: None,
        })
    }

    /// Bind the UDP sockets and spawn the GVCP/GVSP tasks.
    ///
    /// Idempotent: calling `start` on an already-started camera is a no-op.
    fn start(&mut self, py: Python<'_>) -> PyResult<()> {
        if self.handle.is_some() {
            return Ok(());
        }
        let width = self.width;
        let height = self.height;
        let fps = self.fps;
        let bind_ip = self.bind_ip;
        let port = self.port;
        let pixel_format = self.pixel_format;
        let built = py.allow_threads(|| {
            runtime().block_on(async move {
                FakeCamera::builder()
                    .width(width)
                    .height(height)
                    .fps(fps)
                    .bind_ip(bind_ip)
                    .port(port)
                    .pixel_format(pixel_format)
                    .build()
                    .await
            })
        });
        let cam = built.map_err(|e| transport_error(format!("fake camera: {e}")))?;
        self.port = cam.local_addr().port();
        self.handle = Some(cam);
        Ok(())
    }

    /// Abort the background tasks and release the sockets.
    fn stop(&mut self) {
        self.handle.take();
    }

    /// IP the GVCP socket is bound to.
    #[getter]
    fn ip(&self) -> String {
        self.bind_ip.to_string()
    }

    /// Port the GVCP socket is bound to. Reflects the actual port
    /// assigned by the OS if `port=0` was requested.
    #[getter]
    fn port(&self) -> u16 {
        self.port
    }

    /// Run discovery against the loopback address and return the
    /// matching `GigeDeviceInfo`.
    ///
    /// Raises `TransportError` if the camera isn't running or doesn't
    /// respond within `timeout_ms`.
    #[pyo3(signature = (timeout_ms = 1500))]
    fn device_info(&self, py: Python<'_>, timeout_ms: u64) -> PyResult<PyGigeDeviceInfo> {
        if self.handle.is_none() {
            return Err(transport_error(
                "fake camera is not running; call start() or use as a context manager",
            ));
        }
        let target_ip = self.bind_ip;
        let devices = py.allow_threads(|| {
            runtime().block_on(async move {
                gige::discover_all(Duration::from_millis(timeout_ms)).await
            })
        });
        let devices = devices.map_err(|e| transport_error(e.to_string()))?;
        devices
            .into_iter()
            .find(|d| d.ip == target_ip)
            .map(PyGigeDeviceInfo::from)
            .ok_or_else(|| {
                transport_error(format!(
                    "fake camera at {target_ip} did not reply to discovery within {timeout_ms} ms"
                ))
            })
    }

    fn __enter__(mut slf: PyRefMut<'_, Self>, py: Python<'_>) -> PyResult<Py<Self>> {
        slf.start(py)?;
        Ok(slf.into())
    }

    fn __exit__(
        &mut self,
        _exc_type: PyObject,
        _exc: PyObject,
        _tb: PyObject,
    ) -> PyResult<bool> {
        self.stop();
        Ok(false)
    }

    fn __repr__(&self) -> String {
        let state = if self.handle.is_some() { "running" } else { "stopped" };
        format!(
            "FakeGigeCamera({state}, {}x{} @ {} fps on {}:{}, pixel_format=0x{:08x})",
            self.width, self.height, self.fps, self.bind_ip, self.port, self.pixel_format,
        )
    }
}

pub(crate) fn register(py: Python<'_>, parent: &Bound<'_, PyModule>) -> PyResult<()> {
    let submodule = PyModule::new_bound(py, "testing")?;
    submodule.add_class::<PyFakeGigeCamera>()?;
    parent.add_submodule(&submodule)?;
    // Without this, `from viva_genicam._native.testing import ...` fails
    // because Python only searches `sys.modules` for dotted imports.
    py.import_bound("sys")?
        .getattr("modules")?
        .set_item("viva_genicam._native.testing", submodule)?;
    Ok(())
}
