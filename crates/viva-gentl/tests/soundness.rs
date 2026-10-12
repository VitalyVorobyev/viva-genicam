//! The C interface's soundness rules (ADR-0031 §5): handles are validated,
//! not dereferenced; null pointers are refused; no panic unwinds into the
//! caller; `GCGetLastError` reports per thread.

mod common;

use std::ffi::c_void;
use std::ptr::null_mut;

use common::*;
use viva_gentl::abi::*;
use viva_gentl::*;

/// A pointer that is not a handle the producer gave out, nor null.
const BOGUS: Handle = 0xDEAD_BEE0 as Handle;

/// Every handle-taking function answers an unknown handle with
/// `GC_ERR_INVALID_HANDLE` — and, being a key into a registry, the handle is
/// never dereferenced, so this does not crash.
#[test]
fn unknown_handles_are_invalid() {
    let _serial = serial();
    let _lib = Library::init();
    let mut ty = 0;
    let mut size = 0usize;
    let mut count = 0u32;
    let mut handle = GENTL_INVALID_HANDLE;
    let mut changed = 0u8;
    // SAFETY: valid out-pointers; the handles are deliberately bogus.
    unsafe {
        for h in [BOGUS, GENTL_INVALID_HANDLE] {
            assert_eq!(TLClose(h), GC_ERR_INVALID_HANDLE);
            assert_eq!(
                TLGetInfo(h, TL_INFO_ID, &mut ty, null_mut(), &mut size),
                GC_ERR_INVALID_HANDLE
            );
            assert_eq!(TLGetNumInterfaces(h, &mut count), GC_ERR_INVALID_HANDLE);
            assert_eq!(
                TLUpdateInterfaceList(h, &mut changed, 0),
                GC_ERR_INVALID_HANDLE
            );
            assert_eq!(IFClose(h), GC_ERR_INVALID_HANDLE);
            assert_eq!(IFGetNumDevices(h, &mut count), GC_ERR_INVALID_HANDLE);
            assert_eq!(DevClose(h), GC_ERR_INVALID_HANDLE);
            assert_eq!(DevGetPort(h, &mut handle), GC_ERR_INVALID_HANDLE);
            assert_eq!(DSClose(h), GC_ERR_INVALID_HANDLE);
            assert_eq!(DSQueueBuffer(h, h), GC_ERR_INVALID_HANDLE);
            assert_eq!(
                DSStartAcquisition(h, ACQ_START_FLAGS_DEFAULT, 1),
                GC_ERR_INVALID_HANDLE
            );
            let mut data = [0u8; 4];
            let mut len = 4usize;
            assert_eq!(
                GCReadPort(h, 0, data.as_mut_ptr().cast(), &mut len),
                GC_ERR_INVALID_HANDLE
            );
            assert_eq!(GCGetNumPortURLs(h, &mut count), GC_ERR_INVALID_HANDLE);
            assert_eq!(
                GCRegisterEvent(h, EVENT_NEW_BUFFER, &mut handle),
                GC_ERR_INVALID_HANDLE
            );
            assert_eq!(EventKill(h), GC_ERR_INVALID_HANDLE);
            assert_eq!(EventFlush(h), GC_ERR_INVALID_HANDLE);
            let mut event = [0u8; 16];
            let mut event_size = 16usize;
            assert_eq!(
                EventGetData(h, event.as_mut_ptr().cast(), &mut event_size, 0),
                GC_ERR_INVALID_HANDLE
            );
        }
    }
    let (code, text) = last_error();
    assert_eq!(code, GC_ERR_INVALID_HANDLE);
    assert!(!text.is_empty());
}

/// A handle of the wrong kind is invalid for that function: the system is
/// not an interface, a remote port is not a device (`DevClose` on it is
/// `GC_ERR_INVALID_HANDLE`, §6.3.4.1), and a buffer belongs to one stream.
#[test]
fn handles_of_the_wrong_kind_are_invalid() {
    let _serial = serial();
    let _fake = Fake::start();
    let _lib = Library::init();
    let opened = open_fake(DEVICE_ACCESS_CONTROL);
    let remote = remote_port(opened.device);
    let stream = open_stream(opened.device);
    let mut buffer = GENTL_INVALID_HANDLE;
    let mut count = 0u32;
    let mut ty = 0;
    let mut size = 0usize;
    // SAFETY: valid out-pointers; the handles are real but of the wrong kind.
    unsafe {
        check(DSAllocAndAnnounceBuffer(
            stream,
            64,
            null_mut(),
            &mut buffer,
        ));
        assert_eq!(
            IFGetNumDevices(opened.system, &mut count),
            GC_ERR_INVALID_HANDLE
        );
        assert_eq!(
            TLGetNumInterfaces(opened.interface, &mut count),
            GC_ERR_INVALID_HANDLE
        );
        assert_eq!(DevClose(remote), GC_ERR_INVALID_HANDLE);
        assert_eq!(DSClose(opened.device), GC_ERR_INVALID_HANDLE);
        assert_eq!(DSQueueBuffer(stream, stream), GC_ERR_INVALID_HANDLE);
        assert_eq!(
            DSGetBufferInfo(
                opened.device,
                buffer,
                BUFFER_INFO_SIZE,
                &mut ty,
                null_mut(),
                &mut size
            ),
            GC_ERR_INVALID_HANDLE,
            "a buffer is asked of its own stream"
        );
        let mut event = GENTL_INVALID_HANDLE;
        assert_eq!(
            GCRegisterEvent(remote, EVENT_NEW_BUFFER, &mut event),
            GC_ERR_INVALID_HANDLE,
            "the remote port is no event source (§6.3.4.5)"
        );
        assert_eq!(
            GCRegisterEvent(opened.system, EVENT_NEW_BUFFER, &mut event),
            GC_ERR_NOT_IMPLEMENTED,
            "a new-buffer event exists only on a stream"
        );
    }
}

/// Null (and `~0`) pointers are `GC_ERR_INVALID_PARAMETER`, never written.
#[test]
fn null_pointers_are_invalid_parameters() {
    let _serial = serial();
    let _lib = Library::init();
    let all_ones = usize::MAX as *mut Handle;
    // SAFETY: the invalid pointers are refused before any access.
    unsafe {
        assert_eq!(TLOpen(null_mut()), GC_ERR_INVALID_PARAMETER);
        assert_eq!(TLOpen(all_ones), GC_ERR_INVALID_PARAMETER);
        let mut system = GENTL_INVALID_HANDLE;
        check(TLOpen(&mut system));
        assert_eq!(
            TLGetNumInterfaces(system, null_mut()),
            GC_ERR_INVALID_PARAMETER
        );
        let mut ty = 0;
        assert_eq!(
            TLGetInfo(system, TL_INFO_ID, &mut ty, null_mut(), null_mut()),
            GC_ERR_INVALID_PARAMETER
        );
        let mut size = 0usize;
        assert_eq!(
            TLGetInfo(system, TL_INFO_ID, null_mut(), null_mut(), &mut size),
            GC_ERR_INVALID_PARAMETER
        );
        let mut iface = GENTL_INVALID_HANDLE;
        assert_eq!(
            TLOpenInterface(system, std::ptr::null(), &mut iface),
            GC_ERR_INVALID_PARAMETER
        );
        let mut data = [0u8; 4];
        let mut len = 4usize;
        assert_eq!(
            GCReadPort(system, 0, null_mut(), &mut len),
            GC_ERR_INVALID_PARAMETER
        );
        assert_eq!(
            GCReadPort(system, 0, data.as_mut_ptr().cast::<c_void>(), null_mut()),
            GC_ERR_INVALID_PARAMETER
        );
        assert_eq!(
            GCGetLastError(null_mut(), null_mut(), null_mut()),
            GC_ERR_INVALID_PARAMETER
        );
    }
    // The failed GCGetLastError did not overwrite the last error.
    let (code, _) = last_error();
    assert_eq!(code, GC_ERR_INVALID_PARAMETER);
}

/// A panic inside an exported function is caught at the boundary: the call
/// returns `GC_ERR_ERROR`, `GCGetLastError` says what happened, and the
/// library keeps working. Debug builds answer a custom info command by
/// panicking, so the guard every export runs in can be exercised.
#[cfg(debug_assertions)]
#[test]
fn a_panic_does_not_unwind_into_the_caller() {
    let _serial = serial();
    let _lib = Library::init();
    let mut ty = 0;
    let mut size = 0usize;
    // SAFETY: valid pointers.
    let rc = unsafe { GCGetInfo(viva_gentl::PANIC_PROBE, &mut ty, null_mut(), &mut size) };
    assert_eq!(rc, GC_ERR_ERROR);
    let (code, text) = last_error();
    assert_eq!(code, GC_ERR_ERROR);
    assert!(text.contains("panic"), "{text}");
    // Still alive and well.
    let vendor = query_string(|t, b, s| unsafe { GCGetInfo(TL_INFO_VENDOR, t, b, s) });
    assert!(!vendor.is_empty());
}

/// `GCGetLastError` is per thread (§6.3.1.3): a fresh thread has "No Error",
/// and a success does not clear an earlier error.
#[test]
fn the_last_error_is_per_thread() {
    let _serial = serial();
    let _lib = Library::init();
    // SAFETY: deliberately bogus handle, never dereferenced.
    assert_eq!(unsafe { TLClose(BOGUS) }, GC_ERR_INVALID_HANDLE);
    let vendor = query_string(|t, b, s| unsafe { GCGetInfo(TL_INFO_VENDOR, t, b, s) });
    assert!(!vendor.is_empty());
    assert_eq!(
        last_error().0,
        GC_ERR_INVALID_HANDLE,
        "kept across a success"
    );
    let fresh = std::thread::spawn(last_error).join().expect("thread");
    assert_eq!(fresh, (GC_ERR_SUCCESS, "No Error".to_string()));
}

/// The functions the producer does not implement say so (ADR-0031): no
/// composite buffers, flows or buffer parts.
#[test]
fn optional_functions_are_not_implemented() {
    let _serial = serial();
    let _fake = Fake::start();
    let _lib = Library::init();
    let opened = open_fake(DEVICE_ACCESS_CONTROL);
    let stream = open_stream(opened.device);
    let mut buffer = GENTL_INVALID_HANDLE;
    let mut n = 0u32;
    let mut ty = 0;
    let mut size = 0usize;
    // SAFETY: valid pointers.
    unsafe {
        check(DSAllocAndAnnounceBuffer(
            stream,
            64,
            null_mut(),
            &mut buffer,
        ));
        let mut segment = [0u8; 8];
        let mut segments = [segment.as_mut_ptr().cast::<c_void>()];
        let mut sizes = [8usize];
        let mut composite = GENTL_INVALID_HANDLE;
        assert_eq!(
            DSAnnounceCompositeBuffer(
                stream,
                1,
                segments.as_mut_ptr(),
                sizes.as_mut_ptr(),
                null_mut(),
                &mut composite
            ),
            GC_ERR_NOT_IMPLEMENTED
        );
        assert_eq!(
            DSGetNumBufferSegments(stream, buffer, &mut n),
            GC_ERR_NOT_IMPLEMENTED
        );
        assert_eq!(
            DSGetBufferSegmentInfo(stream, buffer, 0, 0, &mut ty, null_mut(), &mut size),
            GC_ERR_NOT_IMPLEMENTED
        );
        assert_eq!(DSGetNumFlows(stream, &mut n), GC_ERR_NOT_IMPLEMENTED);
        assert_eq!(
            DSGetFlowInfo(stream, 0, 0, &mut ty, null_mut(), &mut size),
            GC_ERR_NOT_IMPLEMENTED
        );
        assert_eq!(
            DSGetNumBufferParts(stream, buffer, &mut n),
            GC_ERR_NOT_IMPLEMENTED
        );
        assert_eq!(
            DSGetBufferPartInfo(stream, buffer, 0, 0, &mut ty, null_mut(), &mut size),
            GC_ERR_NOT_IMPLEMENTED
        );
        let mut part = DsBufferPartInfoStacked {
            part_index: 0,
            info_cmd: 0,
            result: 0,
            info_type: 0,
            buffer: null_mut(),
            size: 0,
        };
        assert_eq!(
            DSGetBufferPartInfoStacked(stream, buffer, &mut part, 1),
            GC_ERR_NOT_IMPLEMENTED
        );
        let _ = &mut segment;
    }
}
