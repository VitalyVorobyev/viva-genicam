//! Reading the consumer's arguments and writing the answers back.
//!
//! GenTL §6.1.3: a function filling a buffer "can be called with a NULL
//! pointer for the char* parameter (buffer). The piSize parameter is then
//! filled with the size of buffer needed". Every out-pointer is checked before
//! it is written, and nothing is written on an error, so a failed call leaves
//! the consumer's memory as it was (§6.1.5).

use std::ffi::{CStr, c_char, c_void};

use crate::abi::*;
use crate::error::{Failure, Result, invalid_parameter};

/// An info value and the `INFO_DATATYPE` it travels as.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Info {
    Str(String),
    I32(i32),
    U32(u32),
    U64(u64),
    Ptr(usize),
    Bool(bool),
    SizeT(usize),
}

impl Info {
    pub(crate) fn datatype(&self) -> InfoDatatype {
        match self {
            Info::Str(_) => INFO_DATATYPE_STRING,
            Info::I32(_) => INFO_DATATYPE_INT32,
            Info::U32(_) => INFO_DATATYPE_UINT32,
            Info::U64(_) => INFO_DATATYPE_UINT64,
            Info::Ptr(_) => INFO_DATATYPE_PTR,
            Info::Bool(_) => INFO_DATATYPE_BOOL8,
            Info::SizeT(_) => INFO_DATATYPE_SIZET,
        }
    }

    /// The bytes the consumer receives: numbers in the host's byte order, a
    /// string with its terminating 0 ("the size includes the terminating 0").
    pub(crate) fn bytes(&self) -> Vec<u8> {
        match self {
            Info::Str(s) => {
                let mut bytes = Vec::with_capacity(s.len() + 1);
                bytes.extend(s.bytes().filter(|&b| b != 0));
                bytes.push(0);
                bytes
            }
            Info::I32(v) => v.to_ne_bytes().to_vec(),
            Info::U32(v) => v.to_ne_bytes().to_vec(),
            Info::U64(v) => v.to_ne_bytes().to_vec(),
            Info::Ptr(v) | Info::SizeT(v) => v.to_ne_bytes().to_vec(),
            Info::Bool(v) => vec![u8::from(*v)],
        }
    }
}

/// Whether `p` is a pointer the consumer may have meant: §6.3 calls both
/// `NULL` and `~0x0` invalid.
pub(crate) fn is_valid<T>(p: *const T) -> bool {
    !p.is_null() && p as usize != usize::MAX
}

/// Write `value` to the out-parameter `p`.
///
/// # Safety
///
/// `p`, when valid by [`is_valid`], points to writable memory for a `T`.
pub(crate) unsafe fn write_out<T: Copy>(p: *mut T, value: T, what: &'static str) -> Result<()> {
    if !is_valid(p) {
        return Err(invalid_parameter(what));
    }
    // SAFETY: checked above; the consumer owns the memory (GenTL §6.1.3).
    unsafe { p.write_unaligned(value) };
    Ok(())
}

/// Read the in-out size parameter `p`.
///
/// # Safety
///
/// As [`write_out`].
pub(crate) unsafe fn read_size(p: *const usize, what: &'static str) -> Result<usize> {
    if !is_valid(p) {
        return Err(invalid_parameter(what));
    }
    // SAFETY: checked above.
    Ok(unsafe { p.read_unaligned() })
}

/// Answer an info query: the datatype into `pi_type`, the value into
/// `p_buffer` (or only its size, when `p_buffer` is null) and the bytes
/// written into `pi_size`.
///
/// # Safety
///
/// Valid pointers point to the memory GenTL's info functions describe:
/// `pi_type` an `INFO_DATATYPE`, `pi_size` a `size_t`, `p_buffer` at least
/// `*pi_size` writable bytes.
pub(crate) unsafe fn write_info(
    info: &Info,
    pi_type: *mut InfoDatatype,
    p_buffer: *mut c_void,
    pi_size: *mut usize,
) -> Result<()> {
    if !is_valid(pi_type) || !is_valid(pi_size) {
        return Err(invalid_parameter(
            "piType and piSize must be valid pointers",
        ));
    }
    // SAFETY: the caller's contract on the three pointers.
    unsafe { write_bytes(&info.bytes(), p_buffer.cast(), pi_size)? };
    // SAFETY: checked above.
    unsafe { pi_type.write_unaligned(info.datatype()) };
    Ok(())
}

/// Write `bytes` into `buffer`, which holds `*pi_size` bytes, or, with a null
/// `buffer`, only report how many bytes it needs.
///
/// # Safety
///
/// `pi_size` is valid; a valid `buffer` holds at least `*pi_size` writable
/// bytes.
pub(crate) unsafe fn write_bytes(bytes: &[u8], buffer: *mut u8, pi_size: *mut usize) -> Result<()> {
    if !is_valid(pi_size) {
        return Err(invalid_parameter("piSize must be a valid pointer"));
    }
    if buffer.is_null() {
        // SAFETY: checked above.
        unsafe { pi_size.write_unaligned(bytes.len()) };
        return Ok(());
    }
    if buffer as usize == usize::MAX {
        return Err(invalid_parameter("the buffer pointer is invalid"));
    }
    // SAFETY: checked above.
    let capacity = unsafe { pi_size.read_unaligned() };
    if capacity < bytes.len() {
        return Err(Failure::new(
            GC_ERR_BUFFER_TOO_SMALL,
            format!(
                "the buffer holds {capacity} bytes; {} are needed",
                bytes.len()
            ),
        ));
    }
    // SAFETY: the buffer holds `capacity >= bytes.len()` bytes (caller's
    // contract), and producer data never lives in the consumer's buffer.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len());
        pi_size.write_unaligned(bytes.len());
    }
    Ok(())
}

/// Write `s` as a 0-terminated C string (the `TLGetInterfaceID` family).
///
/// # Safety
///
/// As [`write_bytes`].
pub(crate) unsafe fn write_string(s: &str, buffer: *mut c_char, pi_size: *mut usize) -> Result<()> {
    // SAFETY: forwarded.
    unsafe { write_bytes(&Info::Str(s.to_string()).bytes(), buffer.cast(), pi_size) }
}

/// Read a 0-terminated C string argument (an ID).
///
/// # Safety
///
/// A valid `p` points to a 0-terminated string (§6.2.1.4: "A string as an
/// input value without a size parameter must be 0-terminated").
pub(crate) unsafe fn read_cstr(p: *const c_char, what: &'static str) -> Result<String> {
    if !is_valid(p) {
        return Err(invalid_parameter(what));
    }
    // SAFETY: the caller's contract.
    let s = unsafe { CStr::from_ptr(p) };
    Ok(s.to_string_lossy().into_owned())
}
