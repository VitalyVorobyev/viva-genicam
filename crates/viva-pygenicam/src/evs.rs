//! Event-vision blocks: what an event camera streams instead of images.

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};
use viva_genicam::{EvsBlock, EvsFormat};

use crate::frame::{chunks_dict, posix_secs, status_parts};

/// A block is read-only once received, so any thread may hold one.
#[pyclass(name = "EvsBlock", module = "viva_genicam._native", frozen)]
pub(crate) struct PyEvsBlock {
    inner: EvsBlock,
}

impl PyEvsBlock {
    pub(crate) fn new(block: EvsBlock) -> Self {
        Self { inner: block }
    }
}

#[pymethods]
impl PyEvsBlock {
    /// The EVT encoding: `"evt3.0"` or `"evt2.1"`.
    #[getter]
    fn format(&self) -> &'static str {
        match self.inner.format {
            EvsFormat::Evt30 => "evt3.0",
            EvsFormat::Evt21 => "evt2.1",
            _ => "unknown",
        }
    }

    #[getter]
    fn ts_dev(&self) -> Option<u64> {
        self.inner.ts_dev
    }

    #[getter]
    fn ts_host(&self) -> Option<f64> {
        self.inner.ts_host.and_then(posix_secs)
    }

    /// The block id from the transport.
    #[getter]
    fn block_id(&self) -> u64 {
        self.inner.block_id
    }

    /// How the block ended, named as on a frame.
    #[getter]
    fn status(&self) -> &'static str {
        status_parts(self.inner.status).0
    }

    /// Size X from the GVSP leader, as sent.
    #[getter]
    fn leader_size_x(&self) -> u32 {
        self.inner.leader_size_x
    }

    /// Size Y from the GVSP leader, as sent.
    #[getter]
    fn leader_size_y(&self) -> u32 {
        self.inner.leader_size_y
    }

    /// Size Y from the GVSP trailer, as sent.
    #[getter]
    fn trailer_size_y(&self) -> u32 {
        self.inner.trailer_size_y
    }

    /// The block's chunks by chunk ID, or `None` for a block without chunks.
    #[getter]
    fn chunks<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDict>>> {
        self.inner
            .chunks
            .as_ref()
            .map(|chunks| chunks_dict(py, chunks))
            .transpose()
    }

    /// The encoded EVT bytes as transmitted (copies into a Python `bytes`).
    fn payload<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, self.inner.payload.as_ref())
    }

    fn __repr__(&self) -> String {
        format!(
            "EvsBlock(format={}, block_id={}, bytes={})",
            self.format(),
            self.inner.block_id,
            self.inner.payload.len()
        )
    }
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyEvsBlock>()?;
    Ok(())
}
