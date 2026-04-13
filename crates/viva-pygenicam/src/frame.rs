//! Frame → NumPy conversion.

use numpy::{PyArray1, PyArray3, PyArrayMethods};
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use viva_genicam::Frame;
use viva_pfnc::PixelFormat;

use crate::errors::{IntoPyErr, parse_error};

#[pyclass(module = "viva_genicam._native", unsendable)]
pub(crate) struct PyFrame {
    pub(crate) inner: Frame,
}

impl PyFrame {
    pub(crate) fn new(frame: Frame) -> Self {
        Self { inner: frame }
    }
}

fn ts_host_secs(frame: &Frame) -> Option<f64> {
    frame.ts_host.and_then(|t| {
        t.duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs_f64())
    })
}

fn expect_len(actual: usize, expected: usize, label: &str) -> PyResult<()> {
    if actual != expected {
        return Err(parse_error(format!(
            "{label} payload length {actual} does not match expected {expected}"
        )));
    }
    Ok(())
}

#[pymethods]
impl PyFrame {
    #[getter]
    fn width(&self) -> u32 {
        self.inner.width
    }

    #[getter]
    fn height(&self) -> u32 {
        self.inner.height
    }

    #[getter]
    fn pixel_format(&self) -> String {
        format!("{}", self.inner.pixel_format)
    }

    #[getter]
    fn pixel_format_code(&self) -> u32 {
        self.inner.pixel_format.code()
    }

    #[getter]
    fn ts_dev(&self) -> Option<u64> {
        self.inner.ts_dev
    }

    #[getter]
    fn ts_host(&self) -> Option<f64> {
        ts_host_secs(&self.inner)
    }

    /// Raw payload bytes (copies into a Python `bytes` object).
    fn payload<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new_bound(py, self.inner.payload.as_ref())
    }

    /// Convert to a NumPy array in the most natural shape for the pixel format.
    ///
    /// - Mono8   → (H, W) uint8
    /// - Mono16  → (H, W) uint16
    /// - RGB8    → (H, W, 3) uint8
    /// - BGR8    → (H, W, 3) uint8 (reordered to RGB)
    /// - Bayer8  → (H, W, 3) uint8 (demosaiced to RGB)
    /// - Unknown → (N,)     uint8 (raw bytes)
    fn to_numpy<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let h = self.inner.height as usize;
        let w = self.inner.width as usize;
        let payload = self.inner.payload.as_ref();
        match self.inner.pixel_format {
            PixelFormat::Mono8 => {
                expect_len(payload.len(), h * w, "Mono8")?;
                let arr = PyArray1::<u8>::from_slice_bound(py, payload).reshape([h, w])?;
                Ok(arr.into_any())
            }
            PixelFormat::Mono16 => {
                expect_len(payload.len(), h * w * 2, "Mono16")?;
                let u16_vec: Vec<u16> = payload
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                let arr = PyArray1::<u16>::from_vec_bound(py, u16_vec).reshape([h, w])?;
                Ok(arr.into_any())
            }
            PixelFormat::RGB8Packed => {
                expect_len(payload.len(), h * w * 3, "RGB8")?;
                let arr = PyArray1::<u8>::from_slice_bound(py, payload).reshape([h, w, 3])?;
                Ok(arr.into_any())
            }
            PixelFormat::BGR8Packed
            | PixelFormat::BayerRG8
            | PixelFormat::BayerGB8
            | PixelFormat::BayerBG8
            | PixelFormat::BayerGR8 => {
                let rgb = self.inner.to_rgb8().into_py_err()?;
                let arr = PyArray1::<u8>::from_vec_bound(py, rgb).reshape([h, w, 3])?;
                Ok(arr.into_any())
            }
            _ => {
                let arr = PyArray1::<u8>::from_slice_bound(py, payload);
                Ok(arr.into_any())
            }
        }
    }

    /// Always return an (H, W, 3) uint8 RGB NumPy array.
    fn to_rgb8<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyArray3<u8>>> {
        let h = self.inner.height as usize;
        let w = self.inner.width as usize;
        let rgb = self.inner.to_rgb8().into_py_err()?;
        let arr = PyArray1::<u8>::from_vec_bound(py, rgb).reshape([h, w, 3])?;
        Ok(arr)
    }

    fn __repr__(&self) -> String {
        format!(
            "Frame(width={}, height={}, pixel_format={}, bytes={})",
            self.inner.width,
            self.inner.height,
            self.inner.pixel_format,
            self.inner.payload.len()
        )
    }
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyFrame>()?;
    Ok(())
}
