//! Failures, the per-thread last error, and the guard every export runs in.
//!
//! GenTL §6.1.5: "No exception objects may be thrown of any exported
//! function", and a failed call leaves the state as if it had not been made.
//! In Rust the exception is a panic: [`guard`] catches it at the boundary,
//! where unwinding into C would be undefined behaviour, and turns it into
//! `GC_ERR_ERROR` with the panic's message for `GCGetLastError`.

use std::any::Any;
use std::borrow::Cow;
use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};

use viva_genicam::genapi::GenApiError;
use viva_genicam::gige::{GigeError, GigeStatus};
use viva_genicam::{GenicamError, WriteRefused};

use crate::abi::*;

/// A GenTL error code and the text `GCGetLastError` reports for it.
#[derive(Debug, Clone)]
pub(crate) struct Failure {
    pub(crate) code: GcError,
    pub(crate) message: Cow<'static, str>,
}

impl Failure {
    pub(crate) fn new(code: GcError, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// The result of an exported function's body.
pub(crate) type Result<T> = std::result::Result<T, Failure>;

/// Shorthands for the failures every module returns.
pub(crate) fn invalid_parameter(what: &'static str) -> Failure {
    Failure::new(GC_ERR_INVALID_PARAMETER, what)
}

pub(crate) fn invalid_handle(what: &'static str) -> Failure {
    Failure::new(GC_ERR_INVALID_HANDLE, what)
}

pub(crate) fn not_implemented(what: impl Into<Cow<'static, str>>) -> Failure {
    Failure::new(GC_ERR_NOT_IMPLEMENTED, what)
}

pub(crate) fn not_available(what: impl Into<Cow<'static, str>>) -> Failure {
    Failure::new(GC_ERR_NOT_AVAILABLE, what)
}

thread_local! {
    /// `GCGetLastError`'s answer for this thread (§6.3.1.3: "this function
    /// must store this information thread local").
    static LAST_ERROR: RefCell<Option<(GcError, String)>> = const { RefCell::new(None) };
}

/// Record `failure` as this thread's last error.
fn record(failure: &Failure) {
    // A thread being torn down has no last error to keep.
    let _ = LAST_ERROR.try_with(|last| {
        if let Ok(mut last) = last.try_borrow_mut() {
            *last = Some((failure.code, failure.message.to_string()));
        }
    });
}

/// This thread's last error, or `GC_ERR_SUCCESS` and "No Error" (§6.3.1.3).
pub(crate) fn last_error() -> (GcError, String) {
    LAST_ERROR
        .try_with(|last| last.borrow().clone())
        .ok()
        .flatten()
        .unwrap_or_else(|| (GC_ERR_SUCCESS, "No Error".to_string()))
}

/// Run an exported function's body: its failure, or a panic inside it,
/// becomes the returned code and this thread's last error.
pub(crate) fn guard(body: impl FnOnce() -> Result<()>) -> GcError {
    let failure = match catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(())) => return GC_ERR_SUCCESS,
        Ok(Err(failure)) => failure,
        Err(payload) => panicked(payload),
    };
    record(&failure);
    failure.code
}

/// [`guard`] for `GCGetLastError`, which "will not store the error internally
/// so that succeeding calls to GCGetLastError will still be able to report
/// the stored error code" (§6.3.1.3).
pub(crate) fn guard_unrecorded(body: impl FnOnce() -> Result<()>) -> GcError {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(())) => GC_ERR_SUCCESS,
        Ok(Err(failure)) => failure.code,
        Err(payload) => panicked(payload).code,
    }
}

/// The failure a caught panic becomes.
fn panicked(payload: Box<dyn Any + Send>) -> Failure {
    let message = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic with no message".to_string());
    Failure::new(
        GC_ERR_ERROR,
        format!("internal error: the producer panicked ({message})"),
    )
}

/// A lock poisoned by an earlier panic: the state behind it may be half
/// changed, so it stays an error rather than being used.
pub(crate) fn poisoned<T>(_: T) -> Failure {
    Failure::new(
        GC_ERR_ERROR,
        "internal error: state left inconsistent by an earlier panic",
    )
}

/// Which way a port access went, for mapping a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Read,
    Write,
}

/// Map a register access the camera or the transport refused to its GenTL
/// code, by the error's variant and source (ADR-0030), never by its text.
pub(crate) fn from_genapi(err: &GenApiError, direction: Direction) -> Failure {
    let message = err.to_string();
    let code = match err {
        GenApiError::Io(source) => {
            if let Some(gige) = source.downcast_ref::<GigeError>() {
                gige_code(gige, direction)
            } else if source.downcast_ref::<WriteRefused>().is_some() {
                GC_ERR_ACCESS_DENIED
            } else if let Some(genicam) = source.downcast_ref::<GenicamError>() {
                genicam_code(genicam, direction)
            } else {
                GC_ERR_IO
            }
        }
        _ => GC_ERR_ERROR,
    };
    Failure::new(code, message)
}

/// Map a facade failure to its GenTL code.
pub(crate) fn from_genicam(err: &GenicamError, direction: Direction) -> Failure {
    Failure::new(genicam_code(err, direction), err.to_string())
}

fn genicam_code(err: &GenicamError, direction: Direction) -> GcError {
    match err {
        GenicamError::Gige { source, .. } => gige_code(source, direction),
        GenicamError::GenApi(inner) => from_genapi(inner, direction).code,
        GenicamError::NotInControl(_) => GC_ERR_ACCESS_DENIED,
        GenicamError::Timeout(_) => GC_ERR_TIMEOUT,
        GenicamError::Io(_) | GenicamError::Network { .. } => GC_ERR_IO,
        _ => GC_ERR_ERROR,
    }
}

/// GVCP's answer, in GenTL's terms. A timeout or a socket failure on a port
/// access is GenTL's `GC_ERR_IO` ("a read or write operation to a remote
/// device failed"): port functions list no timeout.
fn gige_code(err: &GigeError, direction: Direction) -> GcError {
    use viva_genicam::gencp::StatusCode;
    match err {
        GigeError::Status(GigeStatus::Common(status)) => match status {
            StatusCode::AccessDenied | StatusCode::WriteProtect => GC_ERR_ACCESS_DENIED,
            StatusCode::InvalidAddress | StatusCode::BadAlignment => GC_ERR_INVALID_ADDRESS,
            StatusCode::InvalidParameter => match direction {
                // "An invalid value has been written" (§6.3.6.6).
                Direction::Write => GC_ERR_INVALID_VALUE,
                Direction::Read => GC_ERR_INVALID_PARAMETER,
            },
            StatusCode::Busy => GC_ERR_BUSY,
            _ => GC_ERR_IO,
        },
        _ => GC_ERR_IO,
    }
}
