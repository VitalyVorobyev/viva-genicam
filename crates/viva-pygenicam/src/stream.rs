//! Sync frame iterator wrapping the async `FrameStream` / `U3vFrameStream`.

use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pyo3::prelude::*;
use tokio::time::timeout as tokio_timeout;

use viva_genicam::gige::nic::Iface;
use viva_genicam::{FrameStream, StreamBuilder, U3vFrameStream as U3vStream, U3vStreamBuilder};

use crate::camera::{GigeCamera, U3vCamera};
use crate::errors::{IntoPyErr, parse_error, transport_error};
use crate::frame::PyFrame;
use crate::runtime::runtime;

enum StreamInner {
    Gige(FrameStream),
    U3v(U3vStream),
}

#[pyclass(module = "viva_genicam._native", unsendable)]
pub(crate) struct PyFrameStream {
    inner: Option<StreamInner>,
    /// Default per-frame timeout when `__next__` is called with no argument.
    default_timeout: Duration,
}

impl PyFrameStream {
    fn gige(stream: FrameStream) -> Self {
        Self {
            inner: Some(StreamInner::Gige(stream)),
            default_timeout: Duration::from_secs(5),
        }
    }

    fn u3v(stream: U3vStream) -> Self {
        Self {
            inner: Some(StreamInner::U3v(stream)),
            default_timeout: Duration::from_secs(5),
        }
    }
}

#[pymethods]
impl PyFrameStream {
    /// Pull the next frame. Returns `None` on clean stream end, raises on error.
    #[pyo3(signature = (timeout_ms=None))]
    fn next_frame(&mut self, py: Python<'_>, timeout_ms: Option<u64>) -> PyResult<Option<PyFrame>> {
        let to = timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(self.default_timeout);
        let inner = self
            .inner
            .as_mut()
            .ok_or_else(|| parse_error("frame stream is closed"))?;
        py.detach(|| match inner {
            StreamInner::Gige(s) => runtime()
                .block_on(async { tokio_timeout(to, s.next_frame()).await })
                .map_err(|_| transport_error("timeout waiting for frame"))
                .and_then(|res| res.into_py_err()),
            StreamInner::U3v(s) => runtime()
                .block_on(async { tokio_timeout(to, s.next_frame()).await })
                .map_err(|_| transport_error("timeout waiting for frame"))
                .and_then(|res| res.into_py_err()),
        })
        .map(|opt| opt.map(PyFrame::new))
    }

    /// Close the stream (Python context manager / explicit shutdown).
    fn close(&mut self) {
        self.inner.take();
    }

    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(mut slf: PyRefMut<'_, Self>, py: Python<'_>) -> PyResult<PyFrame> {
        match slf.next_frame(py, None)? {
            Some(frame) => Ok(frame),
            None => Err(pyo3::exceptions::PyStopIteration::new_err(())),
        }
    }
}

pub(crate) fn build_gige_stream(
    py: Python<'_>,
    camera: Arc<Mutex<GigeCamera>>,
    iface: Option<Iface>,
    auto_packet_size: Option<bool>,
    multicast: Option<Ipv4Addr>,
    destination_port: Option<u16>,
) -> PyResult<PyFrameStream> {
    let time_sync = {
        let g = camera.lock().map_err(|_| parse_error("camera mutex poisoned"))?;
        g.time_sync().clone()
    };

    let stream = py.detach(|| {
        let cam = camera
            .lock()
            .map_err(|_| parse_error("camera mutex poisoned"))?;
        let io = cam.transport();
        let mut device_guard = io.lock_device().map_err(crate::errors::to_py)?;
        let mut builder = StreamBuilder::new(&mut *device_guard);
        if let Some(iface) = iface {
            builder = builder.iface(iface);
        }
        if let Some(enabled) = auto_packet_size {
            builder = builder.auto_packet_size(enabled);
        }
        if let Some(port) = destination_port {
            builder = builder.destination_port(port);
        }
        if let Some(group) = multicast {
            builder = builder.multicast(Some(group));
        }
        runtime().block_on(builder.build()).into_py_err()
    })?;

    let frame_stream = FrameStream::new(stream, Some(time_sync));
    Ok(PyFrameStream::gige(frame_stream))
}

pub(crate) fn build_u3v_stream(
    py: Python<'_>,
    camera: Arc<Mutex<U3vCamera>>,
) -> PyResult<PyFrameStream> {
    let stream = py.detach(|| {
        let mut cam = camera
            .lock()
            .map_err(|_| parse_error("camera mutex poisoned"))?;
        U3vStreamBuilder::new(&mut cam).build().into_py_err()
    })?;
    Ok(PyFrameStream::u3v(stream))
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyFrameStream>()?;
    Ok(())
}
