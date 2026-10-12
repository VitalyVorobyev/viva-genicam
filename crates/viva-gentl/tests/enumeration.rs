//! Interfaces, devices and streams, and what the info functions say about
//! them (GenTL 1.6 §3.2–§3.5, §6.4.1–§6.4.3).

mod common;

use std::ptr::null_mut;

use common::*;
use viva_gentl::abi::*;
use viva_gentl::*;

/// The `TL_INFO_CMD`s the standard marks mandatory, answered alike through
/// `GCGetInfo` and `TLGetInfo` ("must be in sync", §6.4.1.3).
#[test]
fn system_info_is_the_producers() {
    let _serial = serial();
    let _lib = Library::init();
    let mut system = GENTL_INVALID_HANDLE;
    // SAFETY: valid pointers.
    check(unsafe { TLOpen(&mut system) });
    for cmd in [
        TL_INFO_ID,
        TL_INFO_VENDOR,
        TL_INFO_MODEL,
        TL_INFO_VERSION,
        TL_INFO_TLTYPE,
        TL_INFO_NAME,
        TL_INFO_PATHNAME,
        TL_INFO_DISPLAYNAME,
    ] {
        let global = query_string(|t, b, s| unsafe { GCGetInfo(cmd, t, b, s) });
        let module = query_string(|t, b, s| unsafe { TLGetInfo(system, cmd, t, b, s) });
        assert_eq!(global, module, "command {cmd}");
        assert!(!global.is_empty(), "command {cmd}");
    }
    let tl_type = query_string(|t, b, s| unsafe { TLGetInfo(system, TL_INFO_TLTYPE, t, b, s) });
    assert_eq!(tl_type, "GEV");
    let version = query_string(|t, b, s| unsafe { TLGetInfo(system, TL_INFO_VERSION, t, b, s) });
    assert_eq!(version, env!("CARGO_PKG_VERSION"));
    let major = query_u32(|t, b, s| unsafe { TLGetInfo(system, TL_INFO_GENTL_VER_MAJOR, t, b, s) });
    let minor = query_u32(|t, b, s| unsafe { TLGetInfo(system, TL_INFO_GENTL_VER_MINOR, t, b, s) });
    assert_eq!((major, minor), (1, 6));
    let encoding =
        query_i32(|t, b, s| unsafe { TLGetInfo(system, TL_INFO_CHAR_ENCODING, t, b, s) });
    assert_eq!(encoding, TL_CHAR_ENCODING_UTF8);
    // The path names the loaded library: the test binary, linked statically.
    let path = query_string(|t, b, s| unsafe { TLGetInfo(system, TL_INFO_PATHNAME, t, b, s) });
    let name = query_string(|t, b, s| unsafe { TLGetInfo(system, TL_INFO_NAME, t, b, s) });
    assert!(path.ends_with(&name), "{path} ends with {name}");
}

/// An info command the standard does not define is `GC_ERR_NOT_IMPLEMENTED`
/// (§6.1.6), and nothing is written.
#[test]
fn an_unknown_info_command_is_not_implemented() {
    let _serial = serial();
    let _lib = Library::init();
    let mut ty = -7;
    let mut size = 0usize;
    // SAFETY: valid pointers.
    let rc = unsafe { GCGetInfo(999, &mut ty, null_mut(), &mut size) };
    assert_eq!(rc, GC_ERR_NOT_IMPLEMENTED);
    assert_eq!((ty, size), (-7, 0), "nothing written on an error");
}

/// A buffer one byte too small is `GC_ERR_BUFFER_TOO_SMALL`, and the buffer
/// is not touched.
#[test]
fn a_short_buffer_is_refused() {
    let _serial = serial();
    let _lib = Library::init();
    let mut ty = 0;
    let mut size = 0usize;
    // SAFETY: valid pointers.
    unsafe {
        check(GCGetInfo(TL_INFO_VENDOR, &mut ty, null_mut(), &mut size));
        let mut short = vec![0xAAu8; size - 1];
        let mut capacity = short.len();
        let rc = GCGetInfo(
            TL_INFO_VENDOR,
            &mut ty,
            short.as_mut_ptr().cast(),
            &mut capacity,
        );
        assert_eq!(rc, GC_ERR_BUFFER_TOO_SMALL);
        assert!(short.iter().all(|&b| b == 0xAA), "untouched");
    }
}

/// One interface per network interface with an IPv4 address, loopback
/// among them (ADR-0031); IDs from `TLGetInterfaceID` open, and
/// `TLGetInterfaceInfo` agrees with `IFGetInfo`.
#[test]
fn interfaces_are_the_hosts_nics() {
    let _serial = serial();
    let _lib = Library::init();
    let mut system = GENTL_INVALID_HANDLE;
    // SAFETY: valid pointers throughout.
    unsafe {
        check(TLOpen(&mut system));
        let mut changed = 0u8;
        check(TLUpdateInterfaceList(system, &mut changed, 1000));
        let mut count = 0u32;
        check(TLGetNumInterfaces(system, &mut count));
        assert!(count >= 1, "loopback at least");
        let mut names = Vec::new();
        for index in 0..count {
            let iface_id = id(|b, s| TLGetInterfaceID(system, index, b, s));
            let id_c = cstring(&iface_id);
            let tl_type = query_string(|t, b, s| {
                TLGetInterfaceInfo(system, id_c.as_ptr(), INTERFACE_INFO_TLTYPE, t, b, s)
            });
            assert_eq!(tl_type, "GEV");
            let display = query_string(|t, b, s| {
                TLGetInterfaceInfo(system, id_c.as_ptr(), INTERFACE_INFO_DISPLAYNAME, t, b, s)
            });
            let mut iface = GENTL_INVALID_HANDLE;
            check(TLOpenInterface(system, id_c.as_ptr(), &mut iface));
            let mut again = GENTL_INVALID_HANDLE;
            assert_eq!(
                TLOpenInterface(system, id_c.as_ptr(), &mut again),
                GC_ERR_RESOURCE_IN_USE,
                "opened twice"
            );
            let own_id = query_string(|t, b, s| IFGetInfo(iface, INTERFACE_INFO_ID, t, b, s));
            assert_eq!(own_id, iface_id);
            let own_display =
                query_string(|t, b, s| IFGetInfo(iface, INTERFACE_INFO_DISPLAYNAME, t, b, s));
            assert_eq!(own_display, display);
            let mut parent = GENTL_INVALID_HANDLE;
            check(IFGetParentTL(iface, &mut parent));
            assert_eq!(parent, system);
            check(IFClose(iface));
            names.push(iface_id);
        }
        // An index past the list.
        let mut size = 0usize;
        assert_eq!(
            TLGetInterfaceID(system, count, null_mut(), &mut size),
            GC_ERR_INVALID_INDEX
        );
        let unknown = cstring("no-such-interface");
        let mut iface = GENTL_INVALID_HANDLE;
        assert_eq!(
            TLOpenInterface(system, unknown.as_ptr(), &mut iface),
            GC_ERR_INVALID_ID
        );
        assert!(names.iter().all(|n| !n.is_empty()));
    }
}

/// An interface opened by a convenience name — its address — still reports
/// its unique ID (§3.2: "the GenTL Producer must return its unique ID and not
/// the convenience-name").
#[test]
fn an_interface_opened_by_address_reports_its_id() {
    let _serial = serial();
    let _lib = Library::init();
    let mut system = GENTL_INVALID_HANDLE;
    // SAFETY: valid pointers.
    unsafe {
        check(TLOpen(&mut system));
        check(TLUpdateInterfaceList(system, null_mut(), 1000));
        let loopback = cstring("127.0.0.1");
        let mut iface = GENTL_INVALID_HANDLE;
        check(TLOpenInterface(system, loopback.as_ptr(), &mut iface));
        let own_id = query_string(|t, b, s| IFGetInfo(iface, INTERFACE_INFO_ID, t, b, s));
        assert_ne!(own_id, "127.0.0.1");
        let mut count = 0u32;
        check(TLGetNumInterfaces(system, &mut count));
        let listed: Vec<String> = (0..count)
            .map(|i| id(|b, s| TLGetInterfaceID(system, i, b, s)))
            .collect();
        assert!(listed.contains(&own_id), "{own_id} in {listed:?}");
    }
}

/// `IFUpdateDeviceList` finds the fake camera on loopback, and the device
/// info commands report what its discovery acknowledgement says.
#[test]
fn the_fake_camera_is_found_and_described() {
    let _serial = serial();
    let _fake = Fake::start();
    let _lib = Library::init();
    let mut system = GENTL_INVALID_HANDLE;
    // SAFETY: valid pointers throughout.
    unsafe {
        check(TLOpen(&mut system));
        check(TLUpdateInterfaceList(system, null_mut(), 1000));
        let loopback = cstring("127.0.0.1");
        let mut iface = GENTL_INVALID_HANDLE;
        check(TLOpenInterface(system, loopback.as_ptr(), &mut iface));
        let mut changed = 0u8;
        check(IFUpdateDeviceList(iface, &mut changed, 500));
        assert_eq!(changed, 1, "the list changed: it was empty");
        let device_id = fake_device_id(iface);
        let id_c = cstring(&device_id);
        let id_ptr = id_c.as_ptr();
        let info = |cmd| query_string(move |t, b, s| IFGetDeviceInfo(iface, id_ptr, cmd, t, b, s));
        assert_eq!(info(DEVICE_INFO_VENDOR), viva_fake_gige::FAKE_MANUFACTURER);
        assert_eq!(info(DEVICE_INFO_MODEL), viva_fake_gige::FAKE_MODEL);
        assert_eq!(info(DEVICE_INFO_SERIAL_NUMBER), viva_fake_gige::FAKE_SERIAL);
        assert_eq!(
            info(DEVICE_INFO_USER_DEFINED_NAME),
            viva_fake_gige::FAKE_USER_NAME
        );
        assert_eq!(info(DEVICE_INFO_VERSION), viva_fake_gige::FAKE_VERSION);
        assert_eq!(info(DEVICE_INFO_TLTYPE), "GEV");
        assert_eq!(info(DEVICE_INFO_ID), device_id);
        let status = query_i32(|t, b, s| {
            IFGetDeviceInfo(iface, id_c.as_ptr(), DEVICE_INFO_ACCESS_STATUS, t, b, s)
        });
        assert_eq!(status, DEVICE_ACCESS_STATUS_UNKNOWN, "not opened here");

        // A second update with the same camera reports no change.
        check(IFUpdateDeviceList(iface, &mut changed, 500));
        assert_eq!(changed, 0);

        let mut device = GENTL_INVALID_HANDLE;
        check(IFOpenDevice(
            iface,
            id_c.as_ptr(),
            DEVICE_ACCESS_CONTROL,
            &mut device,
        ));
        let mut again = GENTL_INVALID_HANDLE;
        assert_eq!(
            IFOpenDevice(iface, id_c.as_ptr(), DEVICE_ACCESS_CONTROL, &mut again),
            GC_ERR_RESOURCE_IN_USE
        );
        // DevGetInfo is "in sync" with IFGetDeviceInfo (§6.3.4.2).
        for cmd in [
            DEVICE_INFO_ID,
            DEVICE_INFO_VENDOR,
            DEVICE_INFO_MODEL,
            DEVICE_INFO_TLTYPE,
            DEVICE_INFO_DISPLAYNAME,
            DEVICE_INFO_SERIAL_NUMBER,
        ] {
            let listed =
                query_string(|t, b, s| IFGetDeviceInfo(iface, id_c.as_ptr(), cmd, t, b, s));
            let opened = query_string(|t, b, s| DevGetInfo(device, cmd, t, b, s));
            assert_eq!(listed, opened, "command {cmd}");
        }
        let status = query_i32(|t, b, s| DevGetInfo(device, DEVICE_INFO_ACCESS_STATUS, t, b, s));
        assert_eq!(status, DEVICE_ACCESS_STATUS_OPEN_READWRITE);
        let frequency =
            query_u64(|t, b, s| DevGetInfo(device, DEVICE_INFO_TIMESTAMP_FREQUENCY, t, b, s));
        assert_eq!(
            frequency, 1_000_000_000,
            "the fake's default tick frequency"
        );
        let mut parent = GENTL_INVALID_HANDLE;
        check(DevGetParentIF(device, &mut parent));
        assert_eq!(parent, iface);

        // One data stream, its ID fixed, its parent the device.
        let mut streams = 0u32;
        check(DevGetNumDataStreams(device, &mut streams));
        assert_eq!(streams, 1);
        let stream = open_stream(device);
        let mut owner = GENTL_INVALID_HANDLE;
        check(DSGetParentDev(stream, &mut owner));
        assert_eq!(owner, device);
        let stream_id = id(|b, s| DevGetDataStreamID(device, 0, b, s));
        let reported = query_string(|t, b, s| DSGetInfo(stream, STREAM_INFO_ID, t, b, s));
        assert_eq!(reported, stream_id);
        let stream_c = cstring(&stream_id);
        let mut second = GENTL_INVALID_HANDLE;
        assert_eq!(
            DevOpenDataStream(device, stream_c.as_ptr(), &mut second),
            GC_ERR_RESOURCE_IN_USE
        );
        let mut size = 0usize;
        assert_eq!(
            DevGetDataStreamID(device, 1, null_mut(), &mut size),
            GC_ERR_INVALID_INDEX
        );
    }
}

/// A camera opened read-only refuses writes through the remote port with
/// `GC_ERR_ACCESS_DENIED`, and holds no control on the device.
#[test]
fn a_read_only_device_refuses_writes() {
    let _serial = serial();
    let fake = Fake::start();
    let _lib = Library::init();
    let opened = open_fake(DEVICE_ACCESS_READONLY);
    let remote = remote_port(opened.device);
    let ccp = viva_genicam::gige::consts::CONTROL_CHANNEL_PRIVILEGE;
    assert_eq!(fake.peek(ccp, 4), vec![0, 0, 0, 0], "nothing claimed");
    let width = read_port(remote, viva_fake_gige::REG_WIDTH, 4).expect("reads work");
    assert_eq!(u32::from_be_bytes(width.try_into().unwrap()), WIDTH);
    assert_eq!(
        write_port(remote, viva_fake_gige::REG_WIDTH, &32u32.to_be_bytes()),
        Err(GC_ERR_ACCESS_DENIED)
    );
    let status = query_i32(|t, b, s| unsafe {
        DevGetInfo(opened.device, DEVICE_INFO_ACCESS_STATUS, t, b, s)
    });
    assert_eq!(status, DEVICE_ACCESS_STATUS_OPEN_READONLY);
}

/// `DEVICE_ACCESS_EXCLUSIVE` takes exclusive access (CCP bit 0), which the
/// fake enforces: another application cannot even read.
#[test]
fn exclusive_access_is_exclusive() {
    let _serial = serial();
    let fake = Fake::start();
    let _lib = Library::init();
    let _opened = open_fake(DEVICE_ACCESS_EXCLUSIVE);
    let ccp = viva_genicam::gige::consts::CONTROL_CHANNEL_PRIVILEGE;
    let value = u32::from_be_bytes(fake.peek(ccp, 4).try_into().unwrap());
    assert_eq!(
        value & viva_genicam::gige::consts::CCP_EXCLUSIVE,
        viva_genicam::gige::consts::CCP_EXCLUSIVE,
        "CCP {value:#x}"
    );
}
