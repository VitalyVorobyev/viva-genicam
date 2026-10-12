//! Each module's port and the GenApi XML it serves (GenTL 1.6 §4.1, §7), and
//! the remote device port.
//!
//! The XMLs are parsed and evaluated by our own `viva-genapi` here, as a
//! sanity check; Harvesters, with EMVA's reference GenApi, is the independent
//! one (`scripts/gentl-harvesters.py`).

mod common;

use std::ptr::null_mut;

use common::*;
use viva_genicam::genapi::NodeMap;
use viva_gentl::abi::*;
use viva_gentl::*;

/// `PORT_INFO_*` for a module port: the module's name, little-endian, the
/// port name the XML declares.
#[track_caller]
fn check_module_port(port: Handle, module: &str, port_name: &str, xml: &str) {
    let info = |cmd| query_string(move |t, b, s| unsafe { GCGetPortInfo(port, cmd, t, b, s) });
    assert_eq!(info(PORT_INFO_MODULE), module);
    assert_eq!(info(PORT_INFO_TLTYPE), "GEV");
    assert_eq!(info(PORT_INFO_PORTNAME), port_name);
    assert!(!info(PORT_INFO_ID).is_empty());
    let flag = |cmd| query_bool(move |t, b, s| unsafe { GCGetPortInfo(port, cmd, t, b, s) });
    assert!(flag(PORT_INFO_LITTLE_ENDIAN));
    assert!(!flag(PORT_INFO_BIG_ENDIAN));
    assert!(flag(PORT_INFO_ACCESS_READ));
    assert!(!flag(PORT_INFO_ACCESS_NA));
    assert!(!flag(PORT_INFO_ACCESS_NI));
    assert!(
        xml.contains(&format!("<Port Name=\"{port_name}\"")),
        "the XML declares the port {port_name}"
    );
    // The GenApi 1.1 namespace EMVA's reference GenApi requires.
    assert!(xml.contains("xmlns=\"http://www.genicam.org/GenApi/Version_1_1\""));
    let scheme = query_fixed::<4>(INFO_DATATYPE_INT32, |t, b, s| unsafe {
        GCGetPortURLInfo(port, 0, URL_INFO_SCHEME, t, b, s)
    });
    assert_eq!(i32::from_ne_bytes(scheme), URL_SCHEME_LOCAL);
    let (file, address, length) = local_url(&port_url(port));
    let reported_address = query_u64(|t, b, s| unsafe {
        GCGetPortURLInfo(port, 0, URL_INFO_FILE_REGISTER_ADDRESS, t, b, s)
    });
    let reported_size =
        query_u64(|t, b, s| unsafe { GCGetPortURLInfo(port, 0, URL_INFO_FILE_SIZE, t, b, s) });
    assert_eq!((reported_address, reported_size), (address, length as u64));
    let filename =
        query_string(|t, b, s| unsafe { GCGetPortURLInfo(port, 0, URL_INFO_FILENAME, t, b, s) });
    assert_eq!(filename, file);
    let schema_major = query_i32(|t, b, s| unsafe {
        GCGetPortURLInfo(port, 0, URL_INFO_SCHEMA_VER_MAJOR, t, b, s)
    });
    let schema_minor = query_i32(|t, b, s| unsafe {
        GCGetPortURLInfo(port, 0, URL_INFO_SCHEMA_VER_MINOR, t, b, s)
    });
    assert_eq!((schema_major, schema_minor), (1, 1));
}

#[track_caller]
fn string(map: &NodeMap, port: Handle, name: &str) -> String {
    map.get_string(name, &PortIo(port))
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[track_caller]
fn integer(map: &NodeMap, port: Handle, name: &str) -> i64 {
    map.get_integer(name, &PortIo(port))
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[track_caller]
fn enumeration(map: &NodeMap, port: Handle, name: &str) -> String {
    map.get_enum(name, &PortIo(port))
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// The System module's XML, with GenTL SFNC 1.2's mandatory features, whose
/// values agree with the C interface's.
#[test]
fn the_system_port_serves_its_features() {
    let _serial = serial();
    let _lib = Library::init();
    let mut system = GENTL_INVALID_HANDLE;
    // SAFETY: valid pointers.
    unsafe {
        check(TLOpen(&mut system));
        check(TLUpdateInterfaceList(system, null_mut(), 1000));
    }
    let xml = port_xml(system);
    check_module_port(system, "TLSystem", "TLPort", &xml);
    let map = port_nodemap(system);
    let tl = |cmd| query_string(move |t, b, s| unsafe { TLGetInfo(system, cmd, t, b, s) });
    assert_eq!(string(&map, system, "TLID"), tl(TL_INFO_ID));
    assert_eq!(string(&map, system, "TLVendorName"), tl(TL_INFO_VENDOR));
    assert_eq!(string(&map, system, "TLModelName"), tl(TL_INFO_MODEL));
    assert_eq!(string(&map, system, "TLVersion"), tl(TL_INFO_VERSION));
    assert_eq!(string(&map, system, "TLPath"), tl(TL_INFO_PATHNAME));
    assert_eq!(enumeration(&map, system, "TLType"), "GigEVision");
    assert_eq!(integer(&map, system, "GenTLVersionMajor"), 1);
    assert_eq!(integer(&map, system, "GenTLVersionMinor"), 6);
    // InterfaceSelector walks the same list as TLGetInterfaceID.
    let mut count = 0u32;
    // SAFETY: valid pointer.
    check(unsafe { TLGetNumInterfaces(system, &mut count) });
    let mut map = map;
    for index in 0..count {
        map.set_integer("InterfaceSelector", i64::from(index), &PortIo(system))
            .expect("select");
        let listed = id(|b, s| unsafe { TLGetInterfaceID(system, index, b, s) });
        assert_eq!(string(&map, system, "InterfaceID"), listed);
    }
    // InterfaceUpdateList is a command the port executes.
    map.exec_command("InterfaceUpdateList", &PortIo(system))
        .expect("InterfaceUpdateList");
}

/// The Interface module's XML: its own identity, and the device list behind
/// `DeviceSelector`, which `DeviceUpdateList` refreshes.
#[test]
fn the_interface_port_lists_devices() {
    let _serial = serial();
    let _fake = Fake::start();
    let _lib = Library::init();
    let opened = open_fake(DEVICE_ACCESS_CONTROL);
    let iface = opened.interface;
    let xml = port_xml(iface);
    check_module_port(iface, "TLInterface", "InterfacePort", &xml);
    let mut map = port_nodemap(iface);
    let own_id = query_string(|t, b, s| unsafe { IFGetInfo(iface, INTERFACE_INFO_ID, t, b, s) });
    assert_eq!(string(&map, iface, "InterfaceID"), own_id);
    assert_eq!(enumeration(&map, iface, "InterfaceType"), "GigEVision");
    map.exec_command("DeviceUpdateList", &PortIo(iface))
        .expect("DeviceUpdateList");
    let device_id = fake_device_id(iface);
    let mut count = 0u32;
    // SAFETY: valid pointer.
    check(unsafe { IFGetNumDevices(iface, &mut count) });
    let mut found = false;
    for index in 0..count {
        map.set_integer("DeviceSelector", i64::from(index), &PortIo(iface))
            .expect("select");
        if string(&map, iface, "DeviceID") == device_id {
            found = true;
            assert_eq!(
                string(&map, iface, "DeviceModelName"),
                viva_fake_gige::FAKE_MODEL
            );
            assert_eq!(
                string(&map, iface, "DeviceVendorName"),
                viva_fake_gige::FAKE_MANUFACTURER
            );
            assert_eq!(
                enumeration(&map, iface, "DeviceAccessStatus"),
                "OpenReadWrite"
            );
            assert_eq!(
                integer(&map, iface, "GevDeviceIPAddress"),
                i64::from(u32::from(std::net::Ipv4Addr::LOCALHOST))
            );
            let mac = viva_fake_gige::FAKE_MAC
                .iter()
                .fold(0i64, |acc, &b| (acc << 8) | i64::from(b));
            assert_eq!(integer(&map, iface, "GevDeviceMACAddress"), mac);
        }
    }
    assert!(found, "DeviceSelector reaches the fake");
}

/// The Device module's XML: the camera's identity and the stream list.
#[test]
fn the_device_port_describes_the_camera() {
    let _serial = serial();
    let _fake = Fake::start();
    let _lib = Library::init();
    let opened = open_fake(DEVICE_ACCESS_CONTROL);
    let device = opened.device;
    let xml = port_xml(device);
    check_module_port(device, "TLDevice", "DevicePort", &xml);
    let map = port_nodemap(device);
    let dev = |cmd| query_string(move |t, b, s| unsafe { DevGetInfo(device, cmd, t, b, s) });
    assert_eq!(string(&map, device, "DeviceID"), dev(DEVICE_INFO_ID));
    assert_eq!(
        string(&map, device, "DeviceVendorName"),
        dev(DEVICE_INFO_VENDOR)
    );
    assert_eq!(
        string(&map, device, "DeviceModelName"),
        dev(DEVICE_INFO_MODEL)
    );
    assert_eq!(enumeration(&map, device, "DeviceType"), "GigEVision");
    assert_eq!(
        enumeration(&map, device, "DeviceAccessStatus"),
        "OpenReadWrite"
    );
    assert_eq!(
        enumeration(&map, device, "DeviceEndianessMechanism"),
        "Standard"
    );
    let stream_id = id(|b, s| unsafe { DevGetDataStreamID(device, 0, b, s) });
    assert_eq!(string(&map, device, "StreamID"), stream_id);
}

/// The Data Stream module's XML: the buffer counts follow announcements.
#[test]
fn the_stream_port_counts_buffers() {
    let _serial = serial();
    let _fake = Fake::start();
    let _lib = Library::init();
    let opened = open_fake(DEVICE_ACCESS_CONTROL);
    let stream = open_stream(opened.device);
    let xml = port_xml(stream);
    check_module_port(stream, "TLDataStream", "StreamPort", &xml);
    let map = port_nodemap(stream);
    assert_eq!(enumeration(&map, stream, "StreamType"), "GigEVision");
    assert_eq!(
        enumeration(&map, stream, "StreamBufferHandlingMode"),
        "OldestFirst"
    );
    assert_eq!(integer(&map, stream, "StreamAnnouncedBufferCount"), 0);
    let mut buffer = GENTL_INVALID_HANDLE;
    // SAFETY: valid pointers.
    check(unsafe { DSAllocAndAnnounceBuffer(stream, 1024, null_mut(), &mut buffer) });
    assert_eq!(integer(&map, stream, "StreamAnnouncedBufferCount"), 1);
    assert_eq!(integer(&map, stream, "StreamAnnounceBufferMinimum"), 1);
}

/// A Buffer module has no port: every port function on a buffer handle is
/// `GC_ERR_NOT_IMPLEMENTED` (§4.1.1.2).
#[test]
fn a_buffer_has_no_port() {
    let _serial = serial();
    let _fake = Fake::start();
    let _lib = Library::init();
    let opened = open_fake(DEVICE_ACCESS_CONTROL);
    let stream = open_stream(opened.device);
    let mut buffer = GENTL_INVALID_HANDLE;
    // SAFETY: valid pointers.
    unsafe {
        check(DSAllocAndAnnounceBuffer(
            stream,
            1024,
            null_mut(),
            &mut buffer,
        ));
        let mut ty = 0;
        let mut size = 0usize;
        assert_eq!(
            GCGetPortInfo(buffer, PORT_INFO_MODULE, &mut ty, null_mut(), &mut size),
            GC_ERR_NOT_IMPLEMENTED
        );
        let mut count = 0u32;
        assert_eq!(GCGetNumPortURLs(buffer, &mut count), GC_ERR_NOT_IMPLEMENTED);
    }
}

/// The remote device port reads the camera's own registers and XML: its URL
/// is the camera's First URL register, and the document read through it is
/// the fake's XML.
#[test]
fn the_remote_port_is_the_camera() {
    let _serial = serial();
    let fake = Fake::start();
    let _lib = Library::init();
    let opened = open_fake(DEVICE_ACCESS_CONTROL);
    let remote = remote_port(opened.device);
    let first_url = c_text(&fake.peek(viva_genicam::gige::consts::FIRST_URL, 512));
    assert_eq!(port_url(remote), first_url);
    let xml = port_xml(remote);
    assert_eq!(xml.trim_end_matches('\0'), viva_fake_gige::FAKE_XML);
    let info = |cmd| query_string(move |t, b, s| unsafe { GCGetPortInfo(remote, cmd, t, b, s) });
    assert_eq!(info(PORT_INFO_MODULE), "Device");
    assert_eq!(info(PORT_INFO_PORTNAME), "Device");
    let device_id =
        query_string(|t, b, s| unsafe { DevGetInfo(opened.device, DEVICE_INFO_ID, t, b, s) });
    assert_eq!(
        info(PORT_INFO_ID),
        device_id,
        "the local device's ID (§6.4.5.1)"
    );
    let big = query_bool(|t, b, s| unsafe { GCGetPortInfo(remote, PORT_INFO_BIG_ENDIAN, t, b, s) });
    assert!(big, "GigE Vision registers are big-endian");
    let write =
        query_bool(|t, b, s| unsafe { GCGetPortInfo(remote, PORT_INFO_ACCESS_WRITE, t, b, s) });
    assert!(write, "opened with control");

    // Byte-aligned access (§4.1): two bytes from the middle of a register
    // are the same two bytes an aligned read returns.
    let word = read_port(remote, viva_fake_gige::REG_WIDTH, 4).expect("aligned");
    let middle = read_port(remote, viva_fake_gige::REG_WIDTH + 1, 2).expect("unaligned");
    assert_eq!(middle, word[1..3]);

    // A write reaches the device.
    write_port(remote, viva_fake_gige::REG_WIDTH, &32u32.to_be_bytes()).expect("write");
    assert_eq!(fake.peek(viva_fake_gige::REG_WIDTH, 4), 32u32.to_be_bytes());

    // Stacked access (§6.3.6.7–8) does the same entry by entry.
    let mut first = [0u8; 4];
    let mut second = [0u8; 2];
    let mut entries = [
        PortRegisterStackEntry {
            address: viva_fake_gige::REG_WIDTH,
            buffer: first.as_mut_ptr().cast(),
            size: 4,
        },
        PortRegisterStackEntry {
            address: viva_fake_gige::REG_WIDTH + 2,
            buffer: second.as_mut_ptr().cast(),
            size: 2,
        },
    ];
    let mut count = entries.len();
    // SAFETY: each entry's buffer holds its size.
    check(unsafe { GCReadPortStacked(remote, entries.as_mut_ptr(), &mut count) });
    assert_eq!(count, 2);
    assert_eq!(first, 32u32.to_be_bytes());
    assert_eq!(second, [0, 32]);
}
