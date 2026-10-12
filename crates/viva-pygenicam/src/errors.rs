//! Map `GenicamError` variants onto a Python exception hierarchy.

use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use viva_genicam::genapi::GenApiError;
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

/// The Python class for a Rust error, by what went wrong; the message is the
/// Rust error's, which carries its cause.
pub(crate) fn to_py(err: GenicamError) -> PyErr {
    let message = err.to_string();
    match err {
        // Text that does not parse as the feature's type.
        GenicamError::GenApi(GenApiError::InvalidText { .. }) => ParseErrorPy::new_err(message),
        GenicamError::GenApi(_) => GenApiErrorPy::new_err(message),
        GenicamError::Xml(_)
        | GenicamError::Configuration(_)
        | GenicamError::Convert(_)
        | GenicamError::NoBinningFactor { .. }
        | GenicamError::NoBinningEntry { .. }
        | GenicamError::UnsizedFrame { .. } => ParseErrorPy::new_err(message),
        GenicamError::Gige { .. }
        | GenicamError::U3v { .. }
        | GenicamError::Network { .. }
        | GenicamError::Io(_)
        | GenicamError::Timeout(_)
        | GenicamError::NotInControl(_)
        | GenicamError::PacketTooSmall { .. }
        | GenicamError::PacketTooLarge { .. }
        | GenicamError::StreamChannel { .. }
        | GenicamError::PacketDelay { .. }
        | GenicamError::NoInterface
        | GenicamError::InterfaceWithoutIpv4(_)
        | GenicamError::NoRoute { .. }
        | GenicamError::NotMulticast(_)
        | GenicamError::TimeSync(_)
        | GenicamError::Task { .. }
        | GenicamError::Panicked(_) => TransportErrorPy::new_err(message),
        GenicamError::MissingChunkFeature(name) => MissingChunkFeatureErrorPy::new_err(name),
        GenicamError::UnsupportedPixelFormat(fmt) => {
            UnsupportedPixelFormatErrorPy::new_err(fmt.to_string())
        }
        _ => GenicamErrorPy::new_err(message),
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
