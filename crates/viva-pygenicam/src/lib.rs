//! Python bindings for `viva-genicam`.
//!
//! The PyO3 surface here is intentionally thin: it exposes just enough to let
//! the pure-Python facade in `python/viva_genicam/` build an ergonomic
//! Python-native API.

use pyo3::prelude::*;

mod camera;
mod discovery;
mod errors;
mod frame;
mod nodemap;
mod runtime;
mod stream;

#[pymodule]
fn _native(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    errors::register(py, m)?;
    discovery::register(m)?;
    camera::register(m)?;
    frame::register(m)?;
    stream::register(m)?;
    nodemap::register(m)?;
    Ok(())
}
