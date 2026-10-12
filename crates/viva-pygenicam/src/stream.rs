//! Sync frame iterator wrapping the async `FrameStream` / `U3vFrameStream`.

use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pyo3::prelude::*;
use pyo3::types::PyDict;
use tokio::time::timeout as tokio_timeout;

use viva_genicam::gige::Iface;
use viva_genicam::{
    FrameStream, GenicamError, ResendPolicy, StreamOptions, StreamStats,
    U3vFrameStream as U3vStream, U3vStreamBuilder,
};

use crate::camera::{lock_open, GigeCamera, U3vCamera};
use crate::errors::{gige_only, parse_error, transport_error, IntoPyErr};
use crate::frame::PyFrame;
use crate::runtime::runtime;

/// Boxed so the enum costs every `PyFrameStream` one pointer whichever
/// transport it streams.
enum StreamInner {
    Gige(Box<FrameStream>),
    U3v(Box<U3vStream>),
}

/// Both streams are `Send + Sync` — the GigE receive engine runs on a thread
/// of its own — so a frame stream may be handed to a worker thread.
#[pyclass(name = "FrameStream", module = "viva_genicam._native")]
pub(crate) struct PyFrameStream {
    inner: Option<StreamInner>,
    /// Default per-frame timeout when `__next__` is called with no argument.
    default_timeout: Duration,
}

impl PyFrameStream {
    fn gige(stream: FrameStream) -> Self {
        Self {
            inner: Some(StreamInner::Gige(Box::new(stream))),
            default_timeout: Duration::from_secs(5),
        }
    }

    fn u3v(stream: U3vStream) -> Self {
        Self {
            inner: Some(StreamInner::U3v(Box::new(stream))),
            default_timeout: Duration::from_secs(5),
        }
    }
}

/// `StreamStats` as a dict of its fields; durations in seconds.
fn stats_dict<'py>(py: Python<'py>, stats: &StreamStats) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    let counters: [(&str, u64); 21] = [
        ("frames_complete", stats.frames_complete),
        ("frames_incomplete", stats.frames_incomplete),
        ("frames_missed", stats.frames_missed),
        ("frames_backpressure", stats.frames_backpressure),
        ("frames_unsupported", stats.frames_unsupported),
        ("packets_received", stats.packets_received),
        ("packets_duplicate", stats.packets_duplicate),
        ("packets_missing", stats.packets_missing),
        ("packets_ignored_malformed", stats.packets_ignored_malformed),
        (
            "packets_ignored_unsupported",
            stats.packets_ignored_unsupported,
        ),
        (
            "packets_ignored_out_of_range",
            stats.packets_ignored_out_of_range,
        ),
        ("packets_ignored_late", stats.packets_ignored_late),
        (
            "packets_ignored_error_status",
            stats.packets_ignored_error_status,
        ),
        ("packets_ignored_foreign", stats.packets_ignored_foreign),
        ("packets_size_mismatch", stats.packets_size_mismatch),
        ("resend_requested", stats.resend_requested),
        ("resend_recovered", stats.resend_recovered),
        ("resend_unavailable", stats.resend_unavailable),
        ("bytes", stats.bytes),
        ("frames_lost", stats.frames_lost()),
        ("packets_ignored", stats.packets_ignored()),
    ];
    for (name, value) in counters {
        dict.set_item(name, value)?;
    }
    dict.set_item("last_frame_dt", stats.last_frame_dt.as_secs_f64())?;
    dict.set_item("avg_fps", stats.avg_fps)?;
    dict.set_item("avg_mbps", stats.avg_mbps)?;
    dict.set_item("avg_latency_ms", stats.avg_latency_ms)?;
    dict.set_item("elapsed", stats.elapsed.as_secs_f64())?;
    dict.set_item("packets_per_second", stats.packets_per_second)?;
    Ok(dict)
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
            // The receive engine's own blocking front end: no runtime needed.
            StreamInner::Gige(s) => match s.next_frame_blocking(to) {
                Err(GenicamError::Timeout(_)) => Err(transport_error("timeout waiting for frame")),
                other => other.into_py_err(),
            },
            StreamInner::U3v(s) => runtime()
                .block_on(async { tokio_timeout(to, s.next_frame()).await })
                .map_err(|_| transport_error("timeout waiting for frame"))
                .and_then(|res| res.into_py_err()),
        })
        .map(|opt| opt.map(PyFrame::new))
    }

    /// The stream's counters (GigE Vision): every block's fate, every
    /// packet's, and the resends asked for.
    fn stats<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        match self.inner.as_ref() {
            Some(StreamInner::Gige(s)) => stats_dict(py, &s.stats()),
            Some(StreamInner::U3v(_)) => Err(gige_only("stream statistics")),
            None => Err(parse_error("frame stream is closed")),
        }
    }

    /// Close the stream (Python context manager / explicit shutdown).
    fn close(&mut self, py: Python<'_>) {
        if let Some(inner) = self.inner.take() {
            // Dropping joins the receive thread: not with the GIL held.
            py.detach(move || drop(inner));
        }
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

/// How Python asked for a GigE stream, beyond the interface.
pub(crate) struct GigeStreamRequest {
    pub packet_size: Option<u32>,
    pub auto_packet_size: bool,
    pub multicast: Option<Ipv4Addr>,
    pub destination_port: Option<u16>,
    pub packet_delay_ns: Option<u64>,
    pub resend: bool,
    pub deliver_incomplete: bool,
    pub monitor: bool,
}

pub(crate) fn build_gige_stream(
    py: Python<'_>,
    camera: Arc<Mutex<Option<GigeCamera>>>,
    iface: Option<Iface>,
    request: GigeStreamRequest,
) -> PyResult<PyFrameStream> {
    let GigeStreamRequest {
        packet_size,
        auto_packet_size,
        multicast,
        destination_port,
        packet_delay_ns,
        resend,
        deliver_incomplete,
        monitor,
    } = request;
    if auto_packet_size && packet_size.is_some() {
        return Err(parse_error(
            "pass at most one of auto_packet_size=True and packet_size=...",
        ));
    }
    if monitor
        && (packet_size.is_some()
            || auto_packet_size
            || multicast.is_some()
            || destination_port.is_some()
            || packet_delay_ns.is_some())
    {
        return Err(parse_error(
            "a monitor watches the stream another application configured: \
             pass no packet size, multicast group, port or packet delay with monitor=True",
        ));
    }

    let frame_stream = py.detach(|| {
        let mut guard = lock_open(&camera)?;
        let cam = guard
            .as_mut()
            .expect("lock_open checked the camera is open");
        let mut options = if monitor {
            StreamOptions::monitor()
        } else {
            StreamOptions::new()
        };
        if let Some(iface) = iface {
            options = options.iface(iface);
        }
        if auto_packet_size {
            options = options.auto_packet_size();
        } else if let Some(size) = packet_size {
            options = options.packet_size(size);
        }
        if let Some(port) = destination_port {
            options = options.destination_port(port);
        }
        if let Some(group) = multicast {
            options = options.multicast(group);
        }
        if let Some(ns) = packet_delay_ns {
            options = options.packet_delay(std::time::Duration::from_nanos(ns));
        }
        if !resend {
            options = options.resend(ResendPolicy::Off);
        }
        let mut stream = cam.open_stream(options).into_py_err()?;
        stream.set_deliver_incomplete(deliver_incomplete);
        Ok::<_, PyErr>(stream)
    })?;
    Ok(PyFrameStream::gige(frame_stream))
}

pub(crate) fn build_u3v_stream(
    py: Python<'_>,
    camera: Arc<Mutex<Option<U3vCamera>>>,
) -> PyResult<PyFrameStream> {
    let stream = py.detach(|| {
        let mut guard = lock_open(&camera)?;
        let cam = guard
            .as_mut()
            .expect("lock_open checked the camera is open");
        // The stream's reader is a runtime task; started outside the runtime
        // it panicked ("no reactor running").
        let _runtime = runtime().enter();
        U3vStreamBuilder::new(cam).build().into_py_err()
    })?;
    Ok(PyFrameStream::u3v(stream))
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyFrameStream>()?;
    Ok(())
}
