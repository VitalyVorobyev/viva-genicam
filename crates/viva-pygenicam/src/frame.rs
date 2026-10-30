//! Frames, and their conversion to NumPy arrays.

use std::time::SystemTime;

use numpy::{PyArray1, PyArray3, PyArrayMethods};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};
use viva_genicam::{ChunkData, Frame, FrameStatus};
use viva_pfnc::convert::{self, ConvertError, Pixels};
use viva_pfnc::PixelFormat;

use crate::errors::{parse_error, IntoPyErr};

/// A frame is read-only once received, so any thread may hold one.
#[pyclass(name = "Frame", module = "viva_genicam._native", frozen)]
pub(crate) struct PyFrame {
    pub(crate) inner: Frame,
}

impl PyFrame {
    pub(crate) fn new(frame: Frame) -> Self {
        Self { inner: frame }
    }
}

/// POSIX seconds, for a timestamp Python reads as a float.
pub(crate) fn posix_secs(time: SystemTime) -> Option<f64> {
    time.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs_f64())
}

/// A pixel format given as a PFNC name or a numeric code.
#[derive(FromPyObject)]
pub(crate) enum PixelFormatArg {
    Code(u32),
    Name(String),
}

impl PixelFormatArg {
    pub(crate) fn resolve(self) -> PyResult<PixelFormat> {
        match self {
            PixelFormatArg::Code(code) => Ok(PixelFormat::from_code(code)),
            PixelFormatArg::Name(name) => match PixelFormat::from_name(&name) {
                PixelFormat::Unknown(_) => Err(parse_error(format!(
                    "unknown pixel format '{name}'; pass a PFNC name or its numeric code"
                ))),
                format => Ok(format),
            },
        }
    }
}

/// What `to_numpy` hands back, before it becomes an array.
enum Decoded {
    Gray8(Vec<u8>),
    Gray16(Vec<u16>),
    Rgb8(Vec<u8>),
    Rgb16(Vec<u16>),
    /// No conversion for the format: the payload's bytes as they are.
    Raw,
}

/// Decode `frame` at the camera's own depth: monochrome and an undemosaiced
/// Bayer mosaic one sample per pixel, everything else RGB. A row stride the
/// leader declared (`padding_x`) is honoured.
fn decode(frame: &Frame, demosaic: bool) -> Result<Decoded, ConvertError> {
    let format = frame.pixel_format;
    let (width, height, stride) = (frame.width, frame.height, frame.row_stride());
    let payload = frame.payload.as_ref();
    if format.is_bayer() && !demosaic {
        let samples = convert::unpack_u16(format, width, height, stride, payload)?;
        return Ok(if format.bits_per_pixel() == Some(8) {
            Decoded::Gray8(samples.into_iter().map(|v| v as u8).collect())
        } else {
            Decoded::Gray16(samples)
        });
    }
    match convert::full_depth(format, width, height, stride, payload) {
        Ok(Pixels::Gray8(samples)) => Ok(Decoded::Gray8(samples)),
        Ok(Pixels::Gray16 { samples, .. }) => Ok(Decoded::Gray16(samples)),
        Ok(Pixels::Rgb8(samples)) => Ok(Decoded::Rgb8(samples)),
        Ok(Pixels::Rgb16 { samples, .. }) => Ok(Decoded::Rgb16(samples)),
        Ok(_) | Err(ConvertError::Unsupported(_)) => Ok(Decoded::Raw),
        Err(err) => Err(err),
    }
}

/// `FrameStatus` as Python sees it: its name, and the missing-packet count or
/// device status code some variants carry.
pub(crate) fn status_parts(status: FrameStatus) -> (&'static str, Option<u32>, Option<u16>) {
    match status {
        FrameStatus::Complete => ("Complete", None, None),
        FrameStatus::Incomplete { missing_packets } => ("Incomplete", Some(missing_packets), None),
        FrameStatus::DeviceStatus(code) => ("DeviceStatus", None, Some(code.to_raw())),
        FrameStatus::TrailerMissing => ("TrailerMissing", None, None),
        _ => ("Unknown", None, None),
    }
}

/// A block's chunks as a dict of chunk ID to bytes.
pub(crate) fn chunks_dict<'py>(
    py: Python<'py>,
    chunks: &ChunkData,
) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    for (id, data) in chunks.iter() {
        dict.set_item(id, PyBytes::new(py, data))?;
    }
    Ok(dict)
}

#[pymethods]
impl PyFrame {
    /// A frame from bytes received or saved elsewhere: `payload` holds the
    /// lines one after another, each followed by `padding_x` bytes.
    #[new]
    #[pyo3(signature = (payload, width, height, pixel_format, padding_x = 0))]
    fn py_new(
        payload: Vec<u8>,
        width: u32,
        height: u32,
        pixel_format: PixelFormatArg,
        padding_x: u16,
    ) -> PyResult<Self> {
        let mut frame = Frame::new(
            bytes::Bytes::from(payload),
            width,
            height,
            pixel_format.resolve()?,
        );
        frame.padding_x = padding_x;
        Ok(Self::new(frame))
    }

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
        self.inner.ts_host.and_then(posix_secs)
    }

    /// The block id from the transport.
    #[getter]
    fn block_id(&self) -> u64 {
        self.inner.block_id
    }

    /// The payload type from the GVSP leader: `0x0001` an image, `0x4001`
    /// an image with chunks.
    #[getter]
    fn payload_type(&self) -> u16 {
        self.inner.payload_type
    }

    /// Bytes after every line of the image, from the GVSP leader.
    #[getter]
    fn padding_x(&self) -> u16 {
        self.inner.padding_x
    }

    /// How the block ended: `"Complete"`, `"Incomplete"`, `"DeviceStatus"`
    /// or `"TrailerMissing"`.
    #[getter]
    fn status(&self) -> &'static str {
        status_parts(self.inner.status).0
    }

    /// Packets that never arrived, for an incomplete frame.
    #[getter]
    fn missing_packets(&self) -> Option<u32> {
        status_parts(self.inner.status).1
    }

    /// The error status the trailer reported, for a `DeviceStatus` frame.
    #[getter]
    fn device_status(&self) -> Option<u16> {
        status_parts(self.inner.status).2
    }

    #[getter]
    fn is_complete(&self) -> bool {
        self.inner.is_complete()
    }

    /// The frame's chunks by chunk ID, or `None` for a frame without chunks.
    #[getter]
    fn chunks<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDict>>> {
        self.inner
            .chunks
            .as_ref()
            .map(|chunks| chunks_dict(py, chunks))
            .transpose()
    }

    /// Raw payload bytes (copies into a Python `bytes` object).
    fn payload<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, self.inner.payload.as_ref())
    }

    /// Convert to a NumPy array at the camera's own depth.
    ///
    /// - 8-bit monochrome → (H, W) uint8
    /// - 10- to 16-bit monochrome, packed or not → (H, W) uint16
    /// - a Bayer mosaic → (H, W), uint8 or uint16, as the camera sent it;
    ///   with `demosaic=True` → (H, W, 3) at the same depth
    /// - RGB, BGR (reordered), with or without alpha, and Y'CbCr → (H, W, 3) uint8
    /// - a format with no conversion → (N,) uint8, the payload's bytes
    #[pyo3(signature = (demosaic = false))]
    fn to_numpy<'py>(&self, py: Python<'py>, demosaic: bool) -> PyResult<Bound<'py, PyAny>> {
        let (h, w) = (self.inner.height as usize, self.inner.width as usize);
        let frame = &self.inner;
        let decoded = py
            .detach(|| decode(frame, demosaic))
            .map_err(|err| parse_error(err.to_string()))?;
        Ok(match decoded {
            Decoded::Gray8(v) => PyArray1::from_vec(py, v).reshape([h, w])?.into_any(),
            Decoded::Gray16(v) => PyArray1::from_vec(py, v).reshape([h, w])?.into_any(),
            Decoded::Rgb8(v) => PyArray1::from_vec(py, v).reshape([h, w, 3])?.into_any(),
            Decoded::Rgb16(v) => PyArray1::from_vec(py, v).reshape([h, w, 3])?.into_any(),
            Decoded::Raw => PyArray1::<u8>::from_slice(py, frame.payload.as_ref()).into_any(),
        })
    }

    /// Always return an (H, W, 3) uint8 RGB NumPy array.
    fn to_rgb8<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyArray3<u8>>> {
        let h = self.inner.height as usize;
        let w = self.inner.width as usize;
        let frame = &self.inner;
        let rgb = py.detach(|| frame.to_rgb8()).into_py_err()?;
        let arr = PyArray1::<u8>::from_vec(py, rgb).reshape([h, w, 3])?;
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
