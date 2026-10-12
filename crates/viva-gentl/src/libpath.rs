//! Where the loaded producer lives: `TL_INFO_PATHNAME` and `TL_INFO_NAME`
//! ("Full path including file name and extension of the library", §6.4.1.3).
//!
//! The consumer loaded us by path, so the operating system knows it: the
//! module that contains one of our own functions.

use std::path::PathBuf;

/// The path of the shared library (or, linked statically into a test, the
/// executable) this code was loaded from.
pub(crate) fn library_path() -> Option<PathBuf> {
    imp::library_path()
}

#[cfg(unix)]
mod imp {
    use std::ffi::{CStr, OsStr};
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;

    pub(super) fn library_path() -> Option<PathBuf> {
        let mut info = std::mem::MaybeUninit::<libc::Dl_info>::zeroed();
        let address = library_path as *const libc::c_void;
        // SAFETY: `dladdr` reads the address and fills `info`; it does not
        // keep either pointer.
        let found = unsafe { libc::dladdr(address, info.as_mut_ptr()) };
        if found == 0 {
            return None;
        }
        // SAFETY: zero-initialised, then filled by a successful `dladdr`.
        let info = unsafe { info.assume_init() };
        if info.dli_fname.is_null() {
            return None;
        }
        // SAFETY: `dli_fname` is a 0-terminated string owned by the loader,
        // valid while the module is loaded — which it is, since this
        // function is in it. It is copied out at once.
        let name = unsafe { CStr::from_ptr(info.dli_fname) };
        let path = PathBuf::from(OsStr::from_bytes(name.to_bytes()));
        Some(std::fs::canonicalize(&path).unwrap_or(path))
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::path::PathBuf;

    use windows_sys::Win32::Foundation::HMODULE;
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleFileNameW, GetModuleHandleExW,
    };

    pub(super) fn library_path() -> Option<PathBuf> {
        let mut module: HMODULE = std::ptr::null_mut();
        let address = library_path as *const u16;
        // SAFETY: with FROM_ADDRESS the "name" is an address inside the
        // module; UNCHANGED_REFCOUNT takes no reference to release.
        let found = unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                    | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                address,
                &mut module,
            )
        };
        if found == 0 {
            return None;
        }
        let mut buffer = vec![0u16; 1024];
        loop {
            // SAFETY: `buffer` holds `len` UTF-16 units.
            let len =
                unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32) }
                    as usize;
            if len == 0 {
                return None;
            }
            if len < buffer.len() {
                return Some(PathBuf::from(OsString::from_wide(&buffer[..len])));
            }
            // Truncated: try a larger buffer, within reason.
            if buffer.len() >= 32 * 1024 {
                return None;
            }
            buffer.resize(buffer.len() * 2, 0);
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    pub(super) fn library_path() -> Option<std::path::PathBuf> {
        None
    }
}
