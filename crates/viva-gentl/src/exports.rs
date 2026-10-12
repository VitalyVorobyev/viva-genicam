//! The 64 functions of the GenTL 1.6 C interface (§6.3), each exported
//! undecorated and run inside [`guard`](crate::error::guard): a failure is
//! returned as its code and kept for `GCGetLastError`, and a panic stops
//! here.
//!
//! Every out-pointer is checked before the call changes anything, so a
//! failed call leaves both the producer and the consumer's memory as they
//! were (§6.1.5).

#![allow(non_snake_case)] // The names are the standard's.

use std::ffi::{c_char, c_void};
use std::sync::Arc;

use crate::abi::*;
use crate::error::{
    Failure, guard, guard_unrecorded, invalid_handle, invalid_parameter, last_error, not_available,
    not_implemented,
};
use crate::library::{self, Library, Object, handle};
use crate::out::{
    Info, is_valid, read_cstr, read_size, write_bytes, write_info, write_out, write_string,
};
use crate::port::{port, url_index};
use crate::stream::Stream;
use crate::system::{self, System};

/// Fail unless `p` may be written: checked before anything changes.
fn writable<T>(p: *mut T, what: &'static str) -> Result<(), Failure> {
    if is_valid(p) {
        Ok(())
    } else {
        Err(invalid_parameter(what))
    }
}

/// An optional out-pointer: null means "do not write" (`pbChanged`,
/// `ppBuffer`), `~0` is invalid.
fn optional<T>(p: *mut T, what: &'static str) -> Result<(), Failure> {
    if p.is_null() || is_valid(p) {
        Ok(())
    } else {
        Err(invalid_parameter(what))
    }
}

/// The stream of a registered new-buffer event.
fn event(lib: &Library, h_event: Handle) -> Result<Arc<Stream>, Failure> {
    match lib.lookup(h_event)?.0 {
        Object::NewBufferEvent(stream) => Ok(stream),
        _ => Err(invalid_handle("not an event handle")),
    }
}

// ---------------------------------------------------------------------------
// §6.3.1 Library functions
// ---------------------------------------------------------------------------

/// `GCGetInfo` (§6.3.1.2): what `TLGetInfo` reports, without a System
/// module.
///
/// # Safety
///
/// Each pointer is null or valid for the access §6.3.1.2 describes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn GCGetInfo(
    i_info_cmd: TlInfoCmd,
    pi_type: *mut InfoDatatype,
    p_buffer: *mut c_void,
    pi_size: *mut usize,
) -> GcError {
    guard(|| {
        library::get()?;
        let info = system::info(i_info_cmd)?;
        // SAFETY: the caller's contract on the three pointers.
        unsafe { write_info(&info, pi_type, p_buffer, pi_size) }
    })
}

/// `GCGetLastError` (§6.3.1.3): this thread's last error; records no error
/// of its own.
///
/// # Safety
///
/// Each pointer is null or valid for the access §6.3.1.3 describes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn GCGetLastError(
    pi_error_code: *mut GcError,
    s_err_text: *mut c_char,
    pi_size: *mut usize,
) -> GcError {
    guard_unrecorded(|| {
        library::get()?;
        writable(pi_error_code, "piErrorCode must be a valid pointer")?;
        let (code, text) = last_error();
        // SAFETY: the caller's contract; both checked before writing.
        unsafe {
            write_string(&text, s_err_text, pi_size)?;
            pi_error_code.write_unaligned(code);
        }
        Ok(())
    })
}

/// `GCInitLib` (§6.3.1.4).
#[unsafe(no_mangle)]
pub extern "C" fn GCInitLib() -> GcError {
    guard(library::init)
}

/// `GCCloseLib` (§6.3.1.1): closes every module still open.
#[unsafe(no_mangle)]
pub extern "C" fn GCCloseLib() -> GcError {
    guard(library::close)
}

// ---------------------------------------------------------------------------
// §6.3.6 Port functions
// ---------------------------------------------------------------------------

/// `GCReadPort` (§6.3.6.5).
///
/// # Safety
///
/// `p_buffer` holds `*pi_size` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn GCReadPort(
    h_port: Handle,
    i_address: u64,
    p_buffer: *mut c_void,
    pi_size: *mut usize,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        writable(p_buffer, "pBuffer must be a valid pointer")?;
        // SAFETY: the caller's contract.
        let len = unsafe { read_size(pi_size, "piSize must be a valid pointer")? };
        let data = port(&lib, h_port)?.read(i_address, len)?;
        // SAFETY: `p_buffer` holds `len` bytes; `data` is exactly `len`.
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), p_buffer.cast::<u8>(), data.len());
            pi_size.write_unaligned(data.len());
        }
        Ok(())
    })
}

/// `GCWritePort` (§6.3.6.6).
///
/// # Safety
///
/// `p_buffer` holds `*pi_size` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn GCWritePort(
    h_port: Handle,
    i_address: u64,
    p_buffer: *const c_void,
    pi_size: *mut usize,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        if !is_valid(p_buffer) {
            return Err(invalid_parameter("pBuffer must be a valid pointer"));
        }
        // SAFETY: the caller's contract.
        let len = unsafe { read_size(pi_size, "piSize must be a valid pointer")? };
        // SAFETY: `p_buffer` holds `len` readable bytes.
        let data = unsafe { std::slice::from_raw_parts(p_buffer.cast::<u8>(), len) };
        port(&lib, h_port)?.write(i_address, data)?;
        // SAFETY: checked by `read_size`.
        unsafe { pi_size.write_unaligned(len) };
        Ok(())
    })
}

/// `GCGetPortURL` (§6.3.6.2, deprecated): the URL list, each 0-terminated,
/// then one more 0.
///
/// # Safety
///
/// `s_url`, when not null, holds `*pi_size` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn GCGetPortURL(
    h_port: Handle,
    s_url: *mut c_char,
    pi_size: *mut usize,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let mut list = Vec::new();
        for url in port(&lib, h_port)?.legacy_urls()? {
            list.extend(url.bytes().filter(|&b| b != 0));
            list.push(0);
        }
        list.push(0);
        // SAFETY: the caller's contract.
        unsafe { write_bytes(&list, s_url.cast(), pi_size) }
    })
}

/// `GCGetPortInfo` (§6.3.6.1).
///
/// # Safety
///
/// Each pointer is null or valid for the access §6.3.6.1 describes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn GCGetPortInfo(
    h_port: Handle,
    i_info_cmd: PortInfoCmd,
    pi_type: *mut InfoDatatype,
    p_buffer: *mut c_void,
    pi_size: *mut usize,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let info = port(&lib, h_port)?.info(i_info_cmd)?;
        // SAFETY: the caller's contract.
        unsafe { write_info(&info, pi_type, p_buffer, pi_size) }
    })
}

/// `GCGetNumPortURLs` (§6.3.6.3).
///
/// # Safety
///
/// `pi_num_urls` is null or valid for writing a `uint32_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn GCGetNumPortURLs(h_port: Handle, pi_num_urls: *mut u32) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let port = port(&lib, h_port)?;
        writable(pi_num_urls, "piNumURLs must be a valid pointer")?;
        let count = u32::try_from(port.urls()?.len()).unwrap_or(u32::MAX);
        // SAFETY: checked above.
        unsafe { write_out(pi_num_urls, count, "piNumURLs") }
    })
}

/// `GCGetPortURLInfo` (§6.3.6.4).
///
/// # Safety
///
/// Each pointer is null or valid for the access §6.3.6.4 describes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn GCGetPortURLInfo(
    h_port: Handle,
    i_url_index: u32,
    i_info_cmd: UrlInfoCmd,
    pi_type: *mut InfoDatatype,
    p_buffer: *mut c_void,
    pi_size: *mut usize,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let urls = port(&lib, h_port)?.urls()?;
        let info = url_index(&urls, i_url_index)?.info(i_info_cmd)?;
        // SAFETY: the caller's contract.
        unsafe { write_info(&info, pi_type, p_buffer, pi_size) }
    })
}

/// `GCReadPortStacked` (§6.3.6.8): on an error, `*pi_num_entries` is how
/// many entries were read.
///
/// # Safety
///
/// `p_entries` holds `*pi_num_entries` entries, each `pBuffer` valid for its
/// `Size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn GCReadPortStacked(
    h_port: Handle,
    p_entries: *mut PortRegisterStackEntry,
    pi_num_entries: *mut usize,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        writable(p_entries, "pEntries must be a valid pointer")?;
        // SAFETY: the caller's contract.
        let count = unsafe { read_size(pi_num_entries, "piNumEntries must be a valid pointer")? };
        let port = port(&lib, h_port)?;
        for index in 0..count {
            // SAFETY: `p_entries` holds `count` entries.
            let entry = unsafe { p_entries.add(index).read_unaligned() };
            let result = if entry.size == 0 {
                Ok(Vec::new())
            } else if is_valid(entry.buffer) {
                port.read(entry.address, entry.size)
            } else {
                Err(invalid_parameter(
                    "an entry's pBuffer is not a valid pointer",
                ))
            };
            match result {
                // SAFETY: the entry's buffer holds `Size` bytes.
                Ok(data) => unsafe {
                    std::ptr::copy_nonoverlapping(
                        data.as_ptr(),
                        entry.buffer.cast::<u8>(),
                        data.len(),
                    );
                },
                Err(failure) => {
                    // SAFETY: checked by `read_size`.
                    unsafe { pi_num_entries.write_unaligned(index) };
                    return Err(failure);
                }
            }
        }
        // SAFETY: checked by `read_size`.
        unsafe { pi_num_entries.write_unaligned(count) };
        Ok(())
    })
}

/// `GCWritePortStacked` (§6.3.6.7): on an error, `*pi_num_entries` is how
/// many entries were written.
///
/// # Safety
///
/// `p_entries` holds `*pi_num_entries` entries, each `pBuffer` valid for its
/// `Size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn GCWritePortStacked(
    h_port: Handle,
    p_entries: *mut PortRegisterStackEntry,
    pi_num_entries: *mut usize,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        writable(p_entries, "pEntries must be a valid pointer")?;
        // SAFETY: the caller's contract.
        let count = unsafe { read_size(pi_num_entries, "piNumEntries must be a valid pointer")? };
        let port = port(&lib, h_port)?;
        for index in 0..count {
            // SAFETY: `p_entries` holds `count` entries.
            let entry = unsafe { p_entries.add(index).read_unaligned() };
            let result = if entry.size == 0 {
                Ok(())
            } else if is_valid(entry.buffer) {
                // SAFETY: the entry's buffer holds `Size` readable bytes.
                let data =
                    unsafe { std::slice::from_raw_parts(entry.buffer.cast::<u8>(), entry.size) };
                port.write(entry.address, data)
            } else {
                Err(invalid_parameter(
                    "an entry's pBuffer is not a valid pointer",
                ))
            };
            if let Err(failure) = result {
                // SAFETY: checked by `read_size`.
                unsafe { pi_num_entries.write_unaligned(index) };
                return Err(failure);
            }
        }
        // SAFETY: checked by `read_size`.
        unsafe { pi_num_entries.write_unaligned(count) };
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// §6.3.7 Signalling functions
// ---------------------------------------------------------------------------

/// `GCRegisterEvent` (§6.3.7.6): `EVENT_NEW_BUFFER` on a data stream; no
/// other event type is implemented.
///
/// # Safety
///
/// `ph_event` is null or valid for writing a handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn GCRegisterEvent(
    h_event_src: Handle,
    i_event_id: EventType,
    ph_event: *mut Handle,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let source = lib.lookup(h_event_src)?.0;
        writable(ph_event, "phEvent must be a valid pointer")?;
        match (source, i_event_id) {
            (Object::Stream(stream), EVENT_NEW_BUFFER) => {
                let key = stream.register_event(&lib)?;
                // SAFETY: checked above.
                unsafe { write_out(ph_event, handle(key), "phEvent") }
            }
            // §6.3.4.5: "The remote device Port handle is no valid source
            // for Events."
            (Object::RemotePort(_) | Object::NewBufferEvent(_), _) => {
                Err(invalid_handle("not a module that can source events"))
            }
            (_, id) => Err(not_implemented(format!("event type {id} on this module"))),
        }
    })
}

/// `GCUnregisterEvent` (§6.3.7.7).
///
/// # Safety
///
/// Takes no pointer the producer dereferences.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn GCUnregisterEvent(h_event_src: Handle, i_event_id: EventType) -> GcError {
    guard(|| {
        let lib = library::get()?;
        match (lib.lookup(h_event_src)?.0, i_event_id) {
            (Object::Stream(stream), EVENT_NEW_BUFFER) => {
                let key = stream.event_key()?.ok_or_else(|| {
                    Failure::new(GC_ERR_NOT_INITIALIZED, "EVENT_NEW_BUFFER is not registered")
                })?;
                lib.forget(key)?;
                stream.unregister_event(key);
                Ok(())
            }
            (Object::RemotePort(_) | Object::NewBufferEvent(_), _) => {
                Err(invalid_handle("not a module that can source events"))
            }
            (_, id) => Err(not_implemented(format!("event type {id} on this module"))),
        }
    })
}

/// `EventGetData` (§6.3.7.2): the next new buffer, as
/// `EVENT_NEW_BUFFER_DATA`.
///
/// # Safety
///
/// `p_buffer`, when not null, holds `*pi_size` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn EventGetData(
    h_event: Handle,
    p_buffer: *mut c_void,
    pi_size: *mut usize,
    i_timeout: u64,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let stream = event(&lib, h_event)?;
        // SAFETY: the caller's contract.
        let capacity = unsafe { read_size(pi_size, "piSize must be a valid pointer")? };
        let needed = std::mem::size_of::<EventNewBufferData>();
        if !p_buffer.is_null() {
            optional(p_buffer, "pBuffer is not a valid pointer")?;
            if capacity < needed {
                return Err(Failure::new(
                    GC_ERR_BUFFER_TOO_SMALL,
                    format!("EVENT_NEW_BUFFER_DATA needs {needed} bytes"),
                ));
            }
        }
        // Checked before a buffer is taken, so a taken one is always handed
        // out: §6.3.7.2, "In case EventGetData returns an error ... no event
        // is removed from the internal queue".
        let (buffer, private) = stream.next_event(h_event as usize, i_timeout)?;
        let data = EventNewBufferData {
            buffer_handle: handle(buffer),
            user_pointer: private as *mut c_void,
        };
        // SAFETY: `p_buffer` holds `needed` bytes; `pi_size` was checked.
        unsafe {
            if p_buffer.is_null() {
                pi_size.write_unaligned(0);
            } else {
                p_buffer.cast::<EventNewBufferData>().write_unaligned(data);
                pi_size.write_unaligned(needed);
            }
        }
        Ok(())
    })
}

/// `EventGetDataInfo` (§6.3.7.3): a field of `EVENT_NEW_BUFFER_DATA`.
///
/// # Safety
///
/// `p_in_buffer` holds `i_in_size` readable bytes; the other pointers are
/// null or valid for the access §6.3.7.3 describes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn EventGetDataInfo(
    h_event: Handle,
    p_in_buffer: *const c_void,
    i_in_size: usize,
    i_info_cmd: EventDataInfoCmd,
    pi_type: *mut InfoDatatype,
    p_out_buffer: *mut c_void,
    pi_out_size: *mut usize,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        event(&lib, h_event)?;
        if !is_valid(p_in_buffer) || i_in_size < std::mem::size_of::<EventNewBufferData>() {
            return Err(invalid_parameter(
                "pInBuffer must hold an EVENT_NEW_BUFFER_DATA",
            ));
        }
        // SAFETY: `p_in_buffer` holds at least one EVENT_NEW_BUFFER_DATA.
        let data = unsafe { p_in_buffer.cast::<EventNewBufferData>().read_unaligned() };
        let info = match i_info_cmd {
            EVENT_DATA_ID => Info::Ptr(data.buffer_handle as usize),
            EVENT_DATA_VALUE => Info::Ptr(data.user_pointer as usize),
            EVENT_DATA_NUMID => {
                return Err(not_available("a buffer handle has no numeric event ID"));
            }
            other => return Err(not_implemented(format!("EVENT_DATA_INFO_CMD {other}"))),
        };
        // SAFETY: the caller's contract.
        unsafe { write_info(&info, pi_type, p_out_buffer, pi_out_size) }
    })
}

/// `EventGetInfo` (§6.3.7.4).
///
/// # Safety
///
/// Each pointer is null or valid for the access §6.3.7.4 describes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn EventGetInfo(
    h_event: Handle,
    i_info_cmd: EventInfoCmd,
    pi_type: *mut InfoDatatype,
    p_buffer: *mut c_void,
    pi_size: *mut usize,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let info = event(&lib, h_event)?.event_info(h_event as usize, i_info_cmd)?;
        // SAFETY: the caller's contract.
        unsafe { write_info(&info, pi_type, p_buffer, pi_size) }
    })
}

/// `EventFlush` (§6.3.7.1): the output queue's buffers back to the
/// consumer, undelivered.
///
/// # Safety
///
/// Takes no pointer the producer dereferences.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn EventFlush(h_event: Handle) -> GcError {
    guard(|| {
        let lib = library::get()?;
        event(&lib, h_event)?.flush_event(h_event as usize)
    })
}

/// `EventKill` (§6.3.7.5): end one wait with `GC_ERR_ABORT`.
///
/// # Safety
///
/// Takes no pointer the producer dereferences.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn EventKill(h_event: Handle) -> GcError {
    guard(|| {
        let lib = library::get()?;
        event(&lib, h_event)?.kill_event(h_event as usize)
    })
}

// ---------------------------------------------------------------------------
// §6.3.2 System functions
// ---------------------------------------------------------------------------

/// `TLOpen` (§6.3.2.6).
///
/// # Safety
///
/// `ph_tl` is null or valid for writing a handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn TLOpen(ph_tl: *mut Handle) -> GcError {
    guard(|| {
        let lib = library::get()?;
        writable(ph_tl, "phSystem must be a valid pointer")?;
        let key = System::open(&lib)?;
        // SAFETY: checked above.
        unsafe { write_out(ph_tl, handle(key), "phSystem") }
    })
}

/// `TLClose` (§6.3.2.1): closes every module under it.
///
/// # Safety
///
/// Takes no pointer the producer dereferences.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn TLClose(h_tl: Handle) -> GcError {
    guard(|| {
        let lib = library::get()?;
        lib.system(h_tl)?;
        lib.close(h_tl as usize)
    })
}

/// `TLGetInfo` (§6.3.2.2).
///
/// # Safety
///
/// Each pointer is null or valid for the access §6.3.2.2 describes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn TLGetInfo(
    h_tl: Handle,
    i_info_cmd: TlInfoCmd,
    pi_type: *mut InfoDatatype,
    p_buffer: *mut c_void,
    pi_size: *mut usize,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        lib.system(h_tl)?;
        let info = system::info(i_info_cmd)?;
        // SAFETY: the caller's contract.
        unsafe { write_info(&info, pi_type, p_buffer, pi_size) }
    })
}

/// `TLGetNumInterfaces` (§6.3.2.5).
///
/// # Safety
///
/// `pi_num_ifaces` is null or valid for writing a `uint32_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn TLGetNumInterfaces(h_tl: Handle, pi_num_ifaces: *mut u32) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let count = lib.system(h_tl)?.interface_count()?;
        // SAFETY: the caller's contract.
        unsafe { write_out(pi_num_ifaces, count, "piNumIfaces must be a valid pointer") }
    })
}

/// `TLGetInterfaceID` (§6.3.2.3).
///
/// # Safety
///
/// `s_id`, when not null, holds `*pi_size` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn TLGetInterfaceID(
    h_tl: Handle,
    i_index: u32,
    s_id: *mut c_char,
    pi_size: *mut usize,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let id = lib.system(h_tl)?.interface_id(i_index)?;
        // SAFETY: the caller's contract.
        unsafe { write_string(&id, s_id, pi_size) }
    })
}

/// `TLGetInterfaceInfo` (§6.3.2.4): by ID, or by a convenience name.
///
/// # Safety
///
/// `s_iface_id` is a 0-terminated string; the other pointers are null or
/// valid for the access §6.3.2.4 describes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn TLGetInterfaceInfo(
    h_tl: Handle,
    s_iface_id: *const c_char,
    i_info_cmd: InterfaceInfoCmd,
    pi_type: *mut InfoDatatype,
    p_buffer: *mut c_void,
    pi_size: *mut usize,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let system = lib.system(h_tl)?;
        // SAFETY: the caller's contract.
        let id = unsafe { read_cstr(s_iface_id, "sIfaceID must be a valid string")? };
        let nic = system.find_interface(&id)?;
        let info = system::interface_info(&nic, i_info_cmd)?;
        // SAFETY: the caller's contract.
        unsafe { write_info(&info, pi_type, p_buffer, pi_size) }
    })
}

/// `TLOpenInterface` (§6.3.2.7).
///
/// # Safety
///
/// `s_iface_id` is a 0-terminated string; `ph_iface` is null or valid for
/// writing a handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn TLOpenInterface(
    h_tl: Handle,
    s_iface_id: *const c_char,
    ph_iface: *mut Handle,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let system = lib.system(h_tl)?;
        // SAFETY: the caller's contract.
        let id = unsafe { read_cstr(s_iface_id, "sIfaceID must be a valid string")? };
        writable(ph_iface, "phIface must be a valid pointer")?;
        let key = system.open_interface(&lib, &id)?;
        // SAFETY: checked above.
        unsafe { write_out(ph_iface, handle(key), "phIface") }
    })
}

/// `TLUpdateInterfaceList` (§6.3.2.8). Listing the host's interfaces does
/// not wait, so `i_timeout` is not needed.
///
/// # Safety
///
/// `pb_changed` is null or valid for writing a `bool8_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn TLUpdateInterfaceList(
    h_tl: Handle,
    pb_changed: *mut Bool8,
    i_timeout: u64,
) -> GcError {
    let _ = i_timeout;
    guard(|| {
        let lib = library::get()?;
        let system = lib.system(h_tl)?;
        optional(pb_changed, "pbChanged is not a valid pointer")?;
        let changed = system.update_interfaces()?;
        if !pb_changed.is_null() {
            // SAFETY: checked above.
            unsafe { pb_changed.write_unaligned(u8::from(changed)) };
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// §6.3.3 Interface functions
// ---------------------------------------------------------------------------

/// `IFClose` (§6.3.3.1): closes every device under it.
///
/// # Safety
///
/// Takes no pointer the producer dereferences.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn IFClose(h_iface: Handle) -> GcError {
    guard(|| {
        let lib = library::get()?;
        lib.interface(h_iface)?;
        lib.close(h_iface as usize)
    })
}

/// `IFGetInfo` (§6.3.3.2).
///
/// # Safety
///
/// Each pointer is null or valid for the access §6.3.3.2 describes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn IFGetInfo(
    h_iface: Handle,
    i_info_cmd: InterfaceInfoCmd,
    pi_type: *mut InfoDatatype,
    p_buffer: *mut c_void,
    pi_size: *mut usize,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let interface = lib.interface(h_iface)?;
        let info = system::interface_info(&interface.nic, i_info_cmd)?;
        // SAFETY: the caller's contract.
        unsafe { write_info(&info, pi_type, p_buffer, pi_size) }
    })
}

/// `IFGetNumDevices` (§6.3.3.5).
///
/// # Safety
///
/// `pi_num_devices` is null or valid for writing a `uint32_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn IFGetNumDevices(h_iface: Handle, pi_num_devices: *mut u32) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let count = lib.interface(h_iface)?.device_count()?;
        // SAFETY: the caller's contract.
        unsafe {
            write_out(
                pi_num_devices,
                count,
                "piNumDevices must be a valid pointer",
            )
        }
    })
}

/// `IFGetDeviceID` (§6.3.3.3).
///
/// # Safety
///
/// `s_device_id`, when not null, holds `*pi_size` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn IFGetDeviceID(
    h_iface: Handle,
    i_index: u32,
    s_device_id: *mut c_char,
    pi_size: *mut usize,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let id = lib.interface(h_iface)?.device_id_at(i_index)?;
        // SAFETY: the caller's contract.
        unsafe { write_string(&id, s_device_id, pi_size) }
    })
}

/// `IFUpdateDeviceList` (§6.3.3.7): GigE Vision discovery on this
/// interface, for `i_timeout` ms (at most 10 s; 1 s for `GENTL_INFINITE`).
///
/// # Safety
///
/// `pb_changed` is null or valid for writing a `bool8_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn IFUpdateDeviceList(
    h_iface: Handle,
    pb_changed: *mut Bool8,
    i_timeout: u64,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let interface = lib.interface(h_iface)?;
        optional(pb_changed, "pbChanged is not a valid pointer")?;
        let changed = interface.update_devices(&lib, i_timeout)?;
        if !pb_changed.is_null() {
            // SAFETY: checked above.
            unsafe { pb_changed.write_unaligned(u8::from(changed)) };
        }
        Ok(())
    })
}

/// `IFGetDeviceInfo` (§6.3.3.4).
///
/// # Safety
///
/// `s_device_id` is a 0-terminated string; the other pointers are null or
/// valid for the access §6.3.3.4 describes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn IFGetDeviceInfo(
    h_iface: Handle,
    s_device_id: *const c_char,
    i_info_cmd: DeviceInfoCmd,
    pi_type: *mut InfoDatatype,
    p_buffer: *mut c_void,
    pi_size: *mut usize,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let interface = lib.interface(h_iface)?;
        // SAFETY: the caller's contract.
        let id = unsafe { read_cstr(s_device_id, "sDeviceID must be a valid string")? };
        let info = interface.device_info(&id, i_info_cmd)?;
        // SAFETY: the caller's contract.
        unsafe { write_info(&info, pi_type, p_buffer, pi_size) }
    })
}

/// `IFOpenDevice` (§6.3.3.6): by ID, or by IPv4 address.
///
/// # Safety
///
/// `s_device_id` is a 0-terminated string; `ph_device` is null or valid for
/// writing a handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn IFOpenDevice(
    h_iface: Handle,
    s_device_id: *const c_char,
    i_open_flag: DeviceAccessFlags,
    ph_device: *mut Handle,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let interface = lib.interface(h_iface)?;
        // SAFETY: the caller's contract.
        let id = unsafe { read_cstr(s_device_id, "sDeviceID must be a valid string")? };
        writable(ph_device, "phDevice must be a valid pointer")?;
        let key = interface.open_device(&lib, &id, i_open_flag)?;
        // SAFETY: checked above.
        unsafe { write_out(ph_device, handle(key), "phDevice") }
    })
}

/// `IFGetParentTL` (§6.3.3.8).
///
/// # Safety
///
/// `ph_system` is null or valid for writing a handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn IFGetParentTL(h_iface: Handle, ph_system: *mut Handle) -> GcError {
    guard(|| {
        let lib = library::get()?;
        lib.interface(h_iface)?;
        let parent = lib.parent(h_iface)?.unwrap_or_default();
        // SAFETY: the caller's contract.
        unsafe {
            write_out(
                ph_system,
                handle(parent),
                "phSystem must be a valid pointer",
            )
        }
    })
}

// ---------------------------------------------------------------------------
// §6.3.4 Device functions
// ---------------------------------------------------------------------------

/// `DevGetPort` (§6.3.4.5): the remote device's port.
///
/// # Safety
///
/// `ph_remote_device` is null or valid for writing a handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DevGetPort(h_device: Handle, ph_remote_device: *mut Handle) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let device = lib.device(h_device)?;
        // SAFETY: the caller's contract.
        unsafe {
            write_out(
                ph_remote_device,
                handle(device.remote_key),
                "phRemoteDev must be a valid pointer",
            )
        }
    })
}

/// `DevGetNumDataStreams` (§6.3.4.4).
///
/// # Safety
///
/// `pi_num_data_streams` is null or valid for writing a `uint32_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DevGetNumDataStreams(
    h_device: Handle,
    pi_num_data_streams: *mut u32,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let count = lib.device(h_device)?.stream_count();
        // SAFETY: the caller's contract.
        unsafe {
            write_out(
                pi_num_data_streams,
                count,
                "piNumDataStreams must be a valid pointer",
            )
        }
    })
}

/// `DevGetDataStreamID` (§6.3.4.3).
///
/// # Safety
///
/// `s_data_stream_id`, when not null, holds `*pi_size` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DevGetDataStreamID(
    h_device: Handle,
    i_index: u32,
    s_data_stream_id: *mut c_char,
    pi_size: *mut usize,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let id = lib.device(h_device)?.stream_id(i_index)?;
        // SAFETY: the caller's contract.
        unsafe { write_string(id, s_data_stream_id, pi_size) }
    })
}

/// `DevOpenDataStream` (§6.3.4.6).
///
/// # Safety
///
/// `s_data_stream_id` is a 0-terminated string; `ph_data_stream` is null or
/// valid for writing a handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DevOpenDataStream(
    h_device: Handle,
    s_data_stream_id: *const c_char,
    ph_data_stream: *mut Handle,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let device = lib.device(h_device)?;
        // SAFETY: the caller's contract.
        let id = unsafe { read_cstr(s_data_stream_id, "sDataStreamID must be a valid string")? };
        writable(ph_data_stream, "phDataStream must be a valid pointer")?;
        let key = device.open_stream(&lib, &id)?;
        // SAFETY: checked above.
        unsafe { write_out(ph_data_stream, handle(key), "phDataStream") }
    })
}

/// `DevGetInfo` (§6.3.4.2).
///
/// # Safety
///
/// Each pointer is null or valid for the access §6.3.4.2 describes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DevGetInfo(
    h_device: Handle,
    i_info_cmd: DeviceInfoCmd,
    pi_type: *mut InfoDatatype,
    p_buffer: *mut c_void,
    pi_size: *mut usize,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let info = lib.device(h_device)?.info(i_info_cmd)?;
        // SAFETY: the caller's contract.
        unsafe { write_info(&info, pi_type, p_buffer, pi_size) }
    })
}

/// `DevClose` (§6.3.4.1): releases the camera. The remote port's handle is
/// not a device's and is refused.
///
/// # Safety
///
/// Takes no pointer the producer dereferences.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DevClose(h_device: Handle) -> GcError {
    guard(|| {
        let lib = library::get()?;
        lib.device(h_device)?;
        lib.close(h_device as usize)
    })
}

/// `DevGetParentIF` (§6.3.4.7).
///
/// # Safety
///
/// `ph_iface` is null or valid for writing a handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DevGetParentIF(h_device: Handle, ph_iface: *mut Handle) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let device = lib.device(h_device)?;
        // SAFETY: the caller's contract.
        unsafe {
            write_out(
                ph_iface,
                handle(device.interface_key),
                "phIface must be a valid pointer",
            )
        }
    })
}

// ---------------------------------------------------------------------------
// §6.3.5 Data stream functions
// ---------------------------------------------------------------------------

/// `DSAnnounceBuffer` (§6.3.5.2): consumer memory, valid until revoked.
///
/// # Safety
///
/// `p_buffer` holds `i_size` bytes and stays valid until the buffer is
/// revoked or the stream closed; `ph_buffer` is null or valid for writing a
/// handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DSAnnounceBuffer(
    h_data_stream: Handle,
    p_buffer: *mut c_void,
    i_size: usize,
    p_private: *mut c_void,
    ph_buffer: *mut Handle,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let stream = lib.stream(h_data_stream)?;
        writable(p_buffer, "pBuffer must be a valid pointer")?;
        writable(ph_buffer, "phBuffer must be a valid pointer")?;
        let key = stream.announce(&lib, Some(p_buffer as usize), i_size, p_private as usize)?;
        // SAFETY: checked above.
        unsafe { write_out(ph_buffer, handle(key), "phBuffer") }
    })
}

/// `DSAllocAndAnnounceBuffer` (§6.3.5.1): the producer's memory.
///
/// # Safety
///
/// `ph_buffer` is null or valid for writing a handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DSAllocAndAnnounceBuffer(
    h_data_stream: Handle,
    i_size: usize,
    p_private: *mut c_void,
    ph_buffer: *mut Handle,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let stream = lib.stream(h_data_stream)?;
        writable(ph_buffer, "phBuffer must be a valid pointer")?;
        let key = stream.announce(&lib, None, i_size, p_private as usize)?;
        // SAFETY: checked above.
        unsafe { write_out(ph_buffer, handle(key), "phBuffer") }
    })
}

/// `DSFlushQueue` (§6.3.5.5): all five `ACQ_QUEUE_TYPE`s.
///
/// # Safety
///
/// Takes no pointer the producer dereferences.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DSFlushQueue(h_data_stream: Handle, i_operation: AcqQueueType) -> GcError {
    guard(|| {
        let lib = library::get()?;
        lib.stream(h_data_stream)?.flush(i_operation)
    })
}

/// `DSStartAcquisition` (§6.3.5.11): opens the camera's stream channel and
/// starts delivering.
///
/// # Safety
///
/// Takes no pointer the producer dereferences.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DSStartAcquisition(
    h_data_stream: Handle,
    i_start_flags: AcqStartFlags,
    i_num_to_acquire: u64,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        lib.stream(h_data_stream)?
            .start(i_start_flags, i_num_to_acquire)
    })
}

/// `DSStopAcquisition` (§6.3.5.12).
///
/// # Safety
///
/// Takes no pointer the producer dereferences.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DSStopAcquisition(
    h_data_stream: Handle,
    i_stop_flags: AcqStopFlags,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        lib.stream(h_data_stream)?.stop(i_stop_flags)
    })
}

/// `DSGetInfo` (§6.3.5.8).
///
/// # Safety
///
/// Each pointer is null or valid for the access §6.3.5.8 describes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DSGetInfo(
    h_data_stream: Handle,
    i_info_cmd: StreamInfoCmd,
    pi_type: *mut InfoDatatype,
    p_buffer: *mut c_void,
    pi_size: *mut usize,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let info = lib.stream(h_data_stream)?.info(i_info_cmd)?;
        // SAFETY: the caller's contract.
        unsafe { write_info(&info, pi_type, p_buffer, pi_size) }
    })
}

/// `DSGetBufferID` (§6.3.5.6): buffers in the order they were announced.
///
/// # Safety
///
/// `ph_buffer` is null or valid for writing a handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DSGetBufferID(
    h_data_stream: Handle,
    i_index: u32,
    ph_buffer: *mut Handle,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let key = lib.stream(h_data_stream)?.buffer_at(i_index)?;
        // SAFETY: the caller's contract.
        unsafe { write_out(ph_buffer, handle(key), "phBuffer must be a valid pointer") }
    })
}

/// `DSClose` (§6.3.5.4): stops the acquisition and revokes every buffer.
///
/// # Safety
///
/// Takes no pointer the producer dereferences.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DSClose(h_data_stream: Handle) -> GcError {
    guard(|| {
        let lib = library::get()?;
        lib.stream(h_data_stream)?;
        lib.close(h_data_stream as usize)
    })
}

/// `DSRevokeBuffer` (§6.3.5.10).
///
/// # Safety
///
/// `p_buffer` and `p_private` are null or valid for writing a pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DSRevokeBuffer(
    h_data_stream: Handle,
    h_buffer: Handle,
    p_buffer: *mut *mut c_void,
    p_private: *mut *mut c_void,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let (stream, key) = lib.buffer(h_data_stream, h_buffer)?;
        optional(p_buffer, "pBuffer is not a valid pointer")?;
        optional(p_private, "pPrivate is not a valid pointer")?;
        let (memory, private) = stream.revoke(&lib, key)?;
        // SAFETY: checked above; null means "ignore".
        unsafe {
            if !p_buffer.is_null() {
                p_buffer.write_unaligned(memory as *mut c_void);
            }
            if !p_private.is_null() {
                p_private.write_unaligned(private as *mut c_void);
            }
        }
        Ok(())
    })
}

/// `DSQueueBuffer` (§6.3.5.9).
///
/// # Safety
///
/// Takes no pointer the producer dereferences.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DSQueueBuffer(h_data_stream: Handle, h_buffer: Handle) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let (stream, key) = lib.buffer(h_data_stream, h_buffer)?;
        stream.queue(key)
    })
}

/// `DSGetBufferInfo` (§6.3.5.7).
///
/// # Safety
///
/// Each pointer is null or valid for the access §6.3.5.7 describes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DSGetBufferInfo(
    h_data_stream: Handle,
    h_buffer: Handle,
    i_info_cmd: BufferInfoCmd,
    pi_type: *mut InfoDatatype,
    p_buffer: *mut c_void,
    pi_size: *mut usize,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let (stream, key) = lib.buffer(h_data_stream, h_buffer)?;
        let info = stream.buffer_info(key, i_info_cmd)?;
        // SAFETY: the caller's contract.
        unsafe { write_info(&info, pi_type, p_buffer, pi_size) }
    })
}

/// `DSGetBufferChunkData` (§6.3.5.13): each chunk's ID, offset and length.
///
/// # Safety
///
/// `p_chunk_data`, when not null, holds `*pi_num_chunks` entries.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DSGetBufferChunkData(
    h_data_stream: Handle,
    h_buffer: Handle,
    p_chunk_data: *mut SingleChunkData,
    pi_num_chunks: *mut usize,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let (stream, key) = lib.buffer(h_data_stream, h_buffer)?;
        // SAFETY: the caller's contract.
        let capacity = unsafe { read_size(pi_num_chunks, "piNumChunks must be a valid pointer")? };
        let chunks = stream.chunks(key)?;
        if !p_chunk_data.is_null() {
            optional(p_chunk_data, "pChunkData is not a valid pointer")?;
            if capacity < chunks.len() {
                return Err(Failure::new(
                    GC_ERR_BUFFER_TOO_SMALL,
                    format!("{} chunks; room for {capacity}", chunks.len()),
                ));
            }
            for (index, &(id, offset, length)) in chunks.iter().enumerate() {
                let entry = SingleChunkData {
                    chunk_id: id,
                    chunk_offset: offset as isize,
                    chunk_length: length,
                };
                // SAFETY: the array holds `capacity >= chunks.len()` entries.
                unsafe { p_chunk_data.add(index).write_unaligned(entry) };
            }
        }
        // SAFETY: checked by `read_size`.
        unsafe { pi_num_chunks.write_unaligned(chunks.len()) };
        Ok(())
    })
}

/// `DSGetParentDev` (§6.3.5.14).
///
/// # Safety
///
/// `ph_device` is null or valid for writing a handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DSGetParentDev(h_data_stream: Handle, ph_device: *mut Handle) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let stream = lib.stream(h_data_stream)?;
        // SAFETY: the caller's contract.
        unsafe {
            write_out(
                ph_device,
                handle(stream.device_key()),
                "phDevice must be a valid pointer",
            )
        }
    })
}

/// `DSGetNumBufferParts` (§6.3.5.15): buffer parts are not implemented.
///
/// # Safety
///
/// Takes no pointer the producer dereferences.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DSGetNumBufferParts(
    h_data_stream: Handle,
    h_buffer: Handle,
    pi_num_parts: *mut u32,
) -> GcError {
    let _ = pi_num_parts;
    guard(|| {
        library::get()?.buffer(h_data_stream, h_buffer)?;
        Err(not_implemented("multi-part buffers are not implemented"))
    })
}

/// `DSGetBufferPartInfo` (§6.3.5.16): buffer parts are not implemented.
///
/// # Safety
///
/// Takes no pointer the producer dereferences.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DSGetBufferPartInfo(
    h_data_stream: Handle,
    h_buffer: Handle,
    i_part_index: u32,
    i_info_cmd: BufferPartInfoCmd,
    pi_type: *mut InfoDatatype,
    p_buffer: *mut c_void,
    pi_size: *mut usize,
) -> GcError {
    let _ = (i_part_index, i_info_cmd, pi_type, p_buffer, pi_size);
    guard(|| {
        library::get()?.buffer(h_data_stream, h_buffer)?;
        Err(not_implemented("multi-part buffers are not implemented"))
    })
}

/// `DSAnnounceCompositeBuffer` (§6.3.5.3): composite buffers are not
/// implemented.
///
/// # Safety
///
/// Takes no pointer the producer dereferences.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DSAnnounceCompositeBuffer(
    h_data_stream: Handle,
    i_num_segments: usize,
    pp_segments: *mut *mut c_void,
    pi_sizes: *mut usize,
    p_private: *mut c_void,
    ph_buffer: *mut Handle,
) -> GcError {
    let _ = (i_num_segments, pp_segments, pi_sizes, p_private, ph_buffer);
    guard(|| {
        library::get()?.stream(h_data_stream)?;
        Err(not_implemented("composite buffers are not implemented"))
    })
}

/// `DSGetBufferInfoStacked` (§6.3.5.17): `DSGetBufferInfo` per entry, each
/// with its own result; the call succeeds unless it fails as a whole.
///
/// # Safety
///
/// `p_info_stacked` holds `i_num_infos` entries, each `pBuffer` null or
/// valid for its `iSize` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DSGetBufferInfoStacked(
    h_data_stream: Handle,
    h_buffer: Handle,
    p_info_stacked: *mut DsBufferInfoStacked,
    i_num_infos: usize,
) -> GcError {
    guard(|| {
        let lib = library::get()?;
        let (stream, key) = lib.buffer(h_data_stream, h_buffer)?;
        if !is_valid(p_info_stacked) || i_num_infos == 0 {
            return Err(invalid_parameter(
                "pInfoStacked must hold at least one entry",
            ));
        }
        for index in 0..i_num_infos {
            // SAFETY: the array holds `i_num_infos` entries.
            let mut entry = unsafe { p_info_stacked.add(index).read_unaligned() };
            let mut info_type = entry.info_type;
            let mut size = entry.size;
            let result = stream.buffer_info(key, entry.info_cmd).and_then(|info| {
                // SAFETY: the entry's buffer, if any, holds `iSize` bytes;
                // the type and size are locals.
                unsafe { write_info(&info, &mut info_type, entry.buffer, &mut size) }
            });
            entry.result = match result {
                Ok(()) => {
                    entry.info_type = info_type;
                    entry.size = size;
                    GC_ERR_SUCCESS
                }
                Err(failure) => failure.code,
            };
            // SAFETY: as above.
            unsafe { p_info_stacked.add(index).write_unaligned(entry) };
        }
        Ok(())
    })
}

/// `DSGetBufferPartInfoStacked` (§6.3.5.18): buffer parts are not
/// implemented.
///
/// # Safety
///
/// Takes no pointer the producer dereferences.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DSGetBufferPartInfoStacked(
    h_data_stream: Handle,
    h_buffer: Handle,
    p_info_stacked: *mut DsBufferPartInfoStacked,
    i_num_infos: usize,
) -> GcError {
    let _ = (p_info_stacked, i_num_infos);
    guard(|| {
        library::get()?.buffer(h_data_stream, h_buffer)?;
        Err(not_implemented("multi-part buffers are not implemented"))
    })
}

/// `DSGetNumFlows` (§6.3.5.19): flows are not implemented.
///
/// # Safety
///
/// Takes no pointer the producer dereferences.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DSGetNumFlows(h_data_stream: Handle, pi_num_flows: *mut u32) -> GcError {
    let _ = pi_num_flows;
    guard(|| {
        library::get()?.stream(h_data_stream)?;
        Err(not_implemented("data stream flows are not implemented"))
    })
}

/// `DSGetFlowInfo` (§6.3.5.20): flows are not implemented.
///
/// # Safety
///
/// Takes no pointer the producer dereferences.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DSGetFlowInfo(
    h_data_stream: Handle,
    i_flow_index: u32,
    i_info_cmd: FlowInfoCmd,
    pi_type: *mut InfoDatatype,
    p_buffer: *mut c_void,
    pi_size: *mut usize,
) -> GcError {
    let _ = (i_flow_index, i_info_cmd, pi_type, p_buffer, pi_size);
    guard(|| {
        library::get()?.stream(h_data_stream)?;
        Err(not_implemented("data stream flows are not implemented"))
    })
}

/// `DSGetNumBufferSegments` (§6.3.5.21): composite buffers are not
/// implemented.
///
/// # Safety
///
/// Takes no pointer the producer dereferences.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DSGetNumBufferSegments(
    h_data_stream: Handle,
    h_buffer: Handle,
    pi_num_segments: *mut u32,
) -> GcError {
    let _ = pi_num_segments;
    guard(|| {
        library::get()?.buffer(h_data_stream, h_buffer)?;
        Err(not_implemented("composite buffers are not implemented"))
    })
}

/// `DSGetBufferSegmentInfo` (§6.3.5.22): composite buffers are not
/// implemented.
///
/// # Safety
///
/// Takes no pointer the producer dereferences.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DSGetBufferSegmentInfo(
    h_data_stream: Handle,
    h_buffer: Handle,
    i_segment_index: u32,
    i_info_cmd: SegmentInfoCmd,
    pi_type: *mut InfoDatatype,
    p_buffer: *mut c_void,
    pi_size: *mut usize,
) -> GcError {
    let _ = (i_segment_index, i_info_cmd, pi_type, p_buffer, pi_size);
    guard(|| {
        library::get()?.buffer(h_data_stream, h_buffer)?;
        Err(not_implemented("composite buffers are not implemented"))
    })
}
