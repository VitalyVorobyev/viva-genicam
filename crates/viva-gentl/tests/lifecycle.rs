//! Opening and closing the library and its modules (GenTL 1.6 §3, §6.3.1).

mod common;

use std::ptr::null_mut;

use common::*;
use viva_gentl::abi::*;
use viva_gentl::*;

/// §3.1: "GCInitLib is called twice ... the second call will return the error
/// GC_ERR_RESOURCE_IN_USE", and the same for `GCCloseLib` without
/// `GCInitLib`; the library opens again after it was closed.
#[test]
fn init_and_close_are_idempotent_and_repeatable() {
    let _serial = serial();
    assert_eq!(GCCloseLib(), GC_ERR_NOT_INITIALIZED, "close before init");
    for _ in 0..3 {
        assert_eq!(GCInitLib(), GC_ERR_SUCCESS);
        assert_eq!(GCInitLib(), GC_ERR_RESOURCE_IN_USE, "second init");
        assert_eq!(GCCloseLib(), GC_ERR_SUCCESS);
        assert_eq!(GCCloseLib(), GC_ERR_NOT_INITIALIZED, "second close");
    }
}

/// Every function needs `GCInitLib` first (`GC_ERR_NOT_INITIALIZED`).
#[test]
fn calls_before_init_are_refused() {
    let _serial = serial();
    let mut system = GENTL_INVALID_HANDLE;
    // SAFETY: valid pointers.
    unsafe {
        assert_eq!(TLOpen(&mut system), GC_ERR_NOT_INITIALIZED);
        let mut ty = 0;
        let mut size = 0usize;
        assert_eq!(
            GCGetInfo(TL_INFO_VENDOR, &mut ty, null_mut(), &mut size),
            GC_ERR_NOT_INITIALIZED
        );
    }
    assert!(system.is_null(), "nothing is written on an error");
}

/// §3.2: one System module per process; a second `TLOpen` is
/// `GC_ERR_RESOURCE_IN_USE`, and after `TLClose` the old handle is invalid
/// and the module opens again.
#[test]
fn the_system_opens_once_and_closes() {
    let _serial = serial();
    let _lib = Library::init();
    let mut system = GENTL_INVALID_HANDLE;
    // SAFETY: valid pointers.
    unsafe {
        check(TLOpen(&mut system));
        assert!(!system.is_null());
        let mut second = GENTL_INVALID_HANDLE;
        assert_eq!(TLOpen(&mut second), GC_ERR_RESOURCE_IN_USE);
        check(TLClose(system));
        assert_eq!(TLClose(system), GC_ERR_INVALID_HANDLE, "closed twice");
        let mut count = 0u32;
        assert_eq!(
            TLGetNumInterfaces(system, &mut count),
            GC_ERR_INVALID_HANDLE
        );
        let mut again = GENTL_INVALID_HANDLE;
        check(TLOpen(&mut again));
        assert_ne!(again, system, "a handle is never reused");
    }
}

/// §6.1.3: "if a Interface module is closed all attached Device, Data Stream
/// and Buffer modules are also closed" — their handles go with it.
#[test]
fn closing_a_parent_closes_its_children() {
    let _serial = serial();
    let _fake = Fake::start();
    let _lib = Library::init();
    let opened = open_fake(DEVICE_ACCESS_CONTROL);
    let remote = remote_port(opened.device);
    let stream = open_stream(opened.device);
    let mut buffer = GENTL_INVALID_HANDLE;
    // SAFETY: valid pointers.
    unsafe {
        check(DSAllocAndAnnounceBuffer(
            stream,
            4096,
            null_mut(),
            &mut buffer,
        ));
        check(IFClose(opened.interface));
        assert_eq!(
            DSClose(stream),
            GC_ERR_INVALID_HANDLE,
            "stream closed with it"
        );
        assert_eq!(DevClose(opened.device), GC_ERR_INVALID_HANDLE);
        let mut count = 0u32;
        assert_eq!(GCGetNumPortURLs(remote, &mut count), GC_ERR_INVALID_HANDLE);
        assert_eq!(DSQueueBuffer(stream, buffer), GC_ERR_INVALID_HANDLE);
        check(TLClose(opened.system));
    }
}

/// Closing a device releases the camera: CCP reads 0 on the fake, so
/// another application can take it at once.
#[test]
fn closing_a_device_releases_control() {
    let _serial = serial();
    let fake = Fake::start();
    let _lib = Library::init();
    let opened = open_fake(DEVICE_ACCESS_CONTROL);
    let ccp = viva_genicam::gige::consts::CONTROL_CHANNEL_PRIVILEGE;
    assert_ne!(
        fake.peek(ccp, 4),
        vec![0, 0, 0, 0],
        "control is held while open"
    );
    // SAFETY: valid handle.
    check(unsafe { DevClose(opened.device) });
    assert_eq!(fake.peek(ccp, 4), vec![0, 0, 0, 0], "released on close");
}

/// §6.3.1.4 and §6.3.1.1 together: `GCCloseLib` closes what the consumer left
/// open, the camera included, and frees every thread it started.
#[test]
fn closing_the_library_closes_everything() {
    let _serial = serial();
    let fake = Fake::start();
    check(GCInitLib());
    let opened = open_fake(DEVICE_ACCESS_CONTROL);
    let _stream = open_stream(opened.device);
    check(GCCloseLib());
    let ccp = viva_genicam::gige::consts::CONTROL_CHANNEL_PRIVILEGE;
    assert_eq!(
        fake.peek(ccp, 4),
        vec![0, 0, 0, 0],
        "released by GCCloseLib"
    );
    // The library opens again, and the old handles mean nothing to it.
    let _lib = Library::init();
    // SAFETY: a stale handle is not dereferenced.
    assert_eq!(unsafe { DevClose(opened.device) }, GC_ERR_INVALID_HANDLE);
}
