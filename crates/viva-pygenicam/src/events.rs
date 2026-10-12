//! GigE Vision events: the GVCP message channel as a blocking iterator.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use viva_genicam::{Event, EventStream};

use crate::errors::{parse_error, transport_error, IntoPyErr};
use crate::frame::posix_secs;
use crate::runtime::runtime;

/// One event the camera sent.
#[pyclass(name = "Event", module = "viva_genicam._native", frozen)]
pub(crate) struct PyEvent {
    pub(crate) inner: Event,
}

#[pymethods]
impl PyEvent {
    /// The event identifier, such as `0x9001` for a camera's own event.
    #[getter]
    fn id(&self) -> u16 {
        self.inner.id
    }

    /// The stream channel the event concerns.
    #[getter]
    fn stream_channel(&self) -> u16 {
        self.inner.stream_channel
    }

    /// The GVSP block the event concerns.
    #[getter]
    fn block_id(&self) -> u64 {
        self.inner.block_id
    }

    /// The device's timestamp, in its ticks.
    #[getter]
    fn ts_dev(&self) -> u64 {
        self.inner.ts_dev
    }

    /// The device timestamp mapped to host time, as POSIX seconds; `None`
    /// until the camera's clock is calibrated (`Camera.time_calibrate`).
    #[getter]
    fn ts_host(&self) -> Option<f64> {
        self.inner.ts_host.and_then(posix_secs)
    }

    /// The event's data after its header; empty for an `EVENT_CMD`.
    #[getter]
    fn data<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.inner.data)
    }

    fn __repr__(&self) -> String {
        format!(
            "Event(id=0x{:04X}, block_id={}, ts_dev={}, data={} bytes)",
            self.inner.id,
            self.inner.block_id,
            self.inner.ts_dev,
            self.inner.data.len()
        )
    }
}

/// The events of one camera, as they arrive.
///
/// The stream is shared, not borrowed, while a thread waits on it, so another
/// thread may close it meanwhile; the wait then ends at its timeout.
#[pyclass(name = "EventStream", module = "viva_genicam._native")]
pub(crate) struct PyEventStream {
    inner: Mutex<Option<Arc<EventStream>>>,
    port: u16,
    default_timeout: Duration,
}

impl PyEventStream {
    pub(crate) fn new(stream: EventStream, port: u16) -> Self {
        Self {
            inner: Mutex::new(Some(Arc::new(stream))),
            port,
            default_timeout: Duration::from_secs(5),
        }
    }

    fn stream(&self) -> PyResult<Arc<EventStream>> {
        self.inner
            .lock()
            .map_err(|_| parse_error("event stream mutex poisoned"))?
            .clone()
            .ok_or_else(|| parse_error("event stream is closed"))
    }
}

#[pymethods]
impl PyEventStream {
    /// The UDP port the events arrive on.
    #[getter]
    fn port(&self) -> u16 {
        self.port
    }

    /// Wait for the next event; raises `TransportError` when none arrives
    /// within `timeout_ms` (default 5 s).
    #[pyo3(signature = (timeout_ms = None))]
    fn next_event(&self, py: Python<'_>, timeout_ms: Option<u64>) -> PyResult<PyEvent> {
        let to = timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(self.default_timeout);
        let stream = self.stream()?;
        let event = py.detach(|| {
            runtime()
                .block_on(async { tokio::time::timeout(to, stream.next()).await })
                .map_err(|_| transport_error("timeout waiting for an event"))
                .and_then(|res| res.into_py_err())
        })?;
        Ok(PyEvent { inner: event })
    }

    /// Stop receiving events and release the socket.
    fn close(&self, py: Python<'_>) -> PyResult<()> {
        let taken = self
            .inner
            .lock()
            .map_err(|_| parse_error("event stream mutex poisoned"))?
            .take();
        py.detach(move || drop(taken));
        Ok(())
    }

    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&self, py: Python<'_>) -> PyResult<PyEvent> {
        self.next_event(py, None)
    }
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyEvent>()?;
    m.add_class::<PyEventStream>()?;
    Ok(())
}
