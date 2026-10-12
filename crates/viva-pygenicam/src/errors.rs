//! Map `GenicamError` variants onto a Python exception hierarchy.

use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use viva_genicam::GenicamError;

/// The exception classes, named — and printed in a traceback — as the
/// `viva_genicam` package exports them.
mod classes {
    use super::PyException;

    pyo3::create_exception!(viva_genicam, GenicamError, PyException);
    pyo3::create_exception!(viva_genicam, GenApiError, GenicamError);
    pyo3::create_exception!(viva_genicam, TransportError, GenicamError);
    pyo3::create_exception!(viva_genicam, ParseError, GenicamError);
    pyo3::create_exception!(viva_genicam, MissingChunkFeatureError, GenicamError);
    pyo3::create_exception!(viva_genicam, UnsupportedPixelFormatError, GenicamError);
}

use classes::{
    GenApiError as GenApiErrorPy, GenicamError as GenicamErrorPy,
    MissingChunkFeatureError as MissingChunkFeatureErrorPy, ParseError as ParseErrorPy,
    TransportError as TransportErrorPy,
    UnsupportedPixelFormatError as UnsupportedPixelFormatErrorPy,
};

pub(crate) fn register(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("GenicamError", py.get_type::<GenicamErrorPy>())?;
    m.add("GenApiError", py.get_type::<GenApiErrorPy>())?;
    m.add("TransportError", py.get_type::<TransportErrorPy>())?;
    m.add("ParseError", py.get_type::<ParseErrorPy>())?;
    m.add(
        "MissingChunkFeatureError",
        py.get_type::<MissingChunkFeatureErrorPy>(),
    )?;
    m.add(
        "UnsupportedPixelFormatError",
        py.get_type::<UnsupportedPixelFormatErrorPy>(),
    )?;
    Ok(())
}

pub(crate) fn to_py(err: GenicamError) -> PyErr {
    match err {
        GenicamError::GenApi(e) => GenApiErrorPy::new_err(e.to_string()),
        GenicamError::Transport(msg) => TransportErrorPy::new_err(msg),
        GenicamError::Parse(msg) => ParseErrorPy::new_err(msg),
        GenicamError::MissingChunkFeature(name) => MissingChunkFeatureErrorPy::new_err(name),
        GenicamError::UnsupportedPixelFormat(fmt) => {
            UnsupportedPixelFormatErrorPy::new_err(fmt.to_string())
        }
        other => GenicamErrorPy::new_err(other.to_string()),
    }
}

/// The base `GenicamError`, for what has no narrower class: a closed camera,
/// a GigE Vision call on a USB3 Vision camera.
pub(crate) fn genicam_error<S: Into<String>>(msg: S) -> PyErr {
    GenicamErrorPy::new_err(msg.into())
}

/// A call only a GigE Vision camera answers.
pub(crate) fn gige_only(what: &str) -> PyErr {
    genicam_error(format!(
        "{what} is GigE Vision only; this is a USB3 Vision camera"
    ))
}

pub(crate) fn transport_error<S: Into<String>>(msg: S) -> PyErr {
    TransportErrorPy::new_err(msg.into())
}

pub(crate) fn parse_error<S: Into<String>>(msg: S) -> PyErr {
    ParseErrorPy::new_err(msg.into())
}

pub(crate) trait IntoPyErr<T> {
    fn into_py_err(self) -> PyResult<T>;
}

impl<T> IntoPyErr<T> for Result<T, GenicamError> {
    fn into_py_err(self) -> PyResult<T> {
        self.map_err(to_py)
    }
}
