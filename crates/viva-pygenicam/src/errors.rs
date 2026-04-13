//! Map `GenicamError` variants onto a Python exception hierarchy.

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use viva_genicam::GenicamError;

create_exception!(_native, GenicamError_, PyException);
create_exception!(_native, GenApiError_, GenicamError_);
create_exception!(_native, TransportError_, GenicamError_);
create_exception!(_native, ParseError_, GenicamError_);
create_exception!(_native, MissingChunkFeatureError_, GenicamError_);
create_exception!(_native, UnsupportedPixelFormatError_, GenicamError_);

pub(crate) fn register(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("GenicamError", py.get_type_bound::<GenicamError_>())?;
    m.add("GenApiError", py.get_type_bound::<GenApiError_>())?;
    m.add("TransportError", py.get_type_bound::<TransportError_>())?;
    m.add("ParseError", py.get_type_bound::<ParseError_>())?;
    m.add(
        "MissingChunkFeatureError",
        py.get_type_bound::<MissingChunkFeatureError_>(),
    )?;
    m.add(
        "UnsupportedPixelFormatError",
        py.get_type_bound::<UnsupportedPixelFormatError_>(),
    )?;
    Ok(())
}

pub(crate) fn to_py(err: GenicamError) -> PyErr {
    match err {
        GenicamError::GenApi(e) => GenApiError_::new_err(e.to_string()),
        GenicamError::Transport(msg) => TransportError_::new_err(msg),
        GenicamError::Parse(msg) => ParseError_::new_err(msg),
        GenicamError::MissingChunkFeature(name) => MissingChunkFeatureError_::new_err(name),
        GenicamError::UnsupportedPixelFormat(fmt) => {
            UnsupportedPixelFormatError_::new_err(fmt.to_string())
        }
        other => GenicamError_::new_err(other.to_string()),
    }
}

pub(crate) fn transport_error<S: Into<String>>(msg: S) -> PyErr {
    TransportError_::new_err(msg.into())
}

pub(crate) fn parse_error<S: Into<String>>(msg: S) -> PyErr {
    ParseError_::new_err(msg.into())
}

pub(crate) trait IntoPyErr<T> {
    fn into_py_err(self) -> PyResult<T>;
}

impl<T> IntoPyErr<T> for Result<T, GenicamError> {
    fn into_py_err(self) -> PyResult<T> {
        self.map_err(to_py)
    }
}
