//! What the producer's integration tests share: calling the exported
//! functions as a consumer would, and a fake camera on loopback.
//!
//! The library is one per process (`GCInitLib` twice is
//! `GC_ERR_RESOURCE_IN_USE`) and every fake camera binds UDP 3956, so each
//! test holds [`serial`] for its whole run.

// Each integration test is its own crate and uses some of these helpers.
#![allow(dead_code)]

use std::ffi::{CString, c_char, c_void};
use std::ptr::null_mut;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use viva_fake_gige::{FakeCamera, FakeCameraBuilder};
use viva_gentl::abi::*;
use viva_gentl::*;

/// One test at a time: the library is per process, the fake's port per host.
pub(crate) fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// This thread's last error, as `GCGetLastError` reports it.
pub(crate) fn last_error() -> (GcError, String) {
    let mut code = GC_ERR_SUCCESS;
    let mut size = 0usize;
    // SAFETY: valid pointers; a null text buffer asks for the size.
    let rc = unsafe { GCGetLastError(&mut code, null_mut(), &mut size) };
    if rc != GC_ERR_SUCCESS {
        return (rc, format!("GCGetLastError itself failed with {rc}"));
    }
    let mut text = vec![0u8; size.max(1)];
    // SAFETY: `text` holds `size` bytes.
    unsafe { GCGetLastError(&mut code, text.as_mut_ptr().cast(), &mut size) };
    (code, c_text(&text))
}

/// Fail the test with the producer's own explanation unless `rc` is success.
#[track_caller]
pub(crate) fn check(rc: GcError) {
    if rc != GC_ERR_SUCCESS {
        let (code, text) = last_error();
        panic!("GenTL call returned {rc}; last error {code}: {text}");
    }
}

/// The text before the first 0 byte.
pub(crate) fn c_text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

pub(crate) fn cstring(s: &str) -> CString {
    CString::new(s).expect("no interior 0")
}

/// `GCInitLib` for the test's lifetime, `GCCloseLib` when it ends.
pub(crate) struct Library;

impl Library {
    #[track_caller]
    pub(crate) fn init() -> Self {
        check(GCInitLib());
        Library
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        GCCloseLib();
    }
}

/// An info query `(piType, pBuffer, piSize) -> rc`: its datatype and bytes,
/// asking for the size first as a consumer does.
pub(crate) fn query(
    f: impl Fn(*mut InfoDatatype, *mut c_void, *mut usize) -> GcError,
) -> Result<(InfoDatatype, Vec<u8>), GcError> {
    let mut ty = INFO_DATATYPE_UNKNOWN;
    let mut size = 0usize;
    let rc = f(&mut ty, null_mut(), &mut size);
    if rc != GC_ERR_SUCCESS {
        return Err(rc);
    }
    let mut buffer = vec![0u8; size];
    let rc = f(&mut ty, buffer.as_mut_ptr().cast(), &mut size);
    if rc != GC_ERR_SUCCESS {
        return Err(rc);
    }
    buffer.truncate(size);
    Ok((ty, buffer))
}

#[track_caller]
pub(crate) fn query_string(
    f: impl Fn(*mut InfoDatatype, *mut c_void, *mut usize) -> GcError,
) -> String {
    let (ty, bytes) =
        query(f).unwrap_or_else(|rc| panic!("query failed: {rc} ({:?})", last_error()));
    assert_eq!(ty, INFO_DATATYPE_STRING, "datatype");
    assert_eq!(
        bytes.last(),
        Some(&0),
        "a string includes its terminating 0"
    );
    c_text(&bytes)
}

/// A fixed-size info value, checked against the datatype the standard gives.
#[track_caller]
pub(crate) fn query_fixed<const N: usize>(
    expected: InfoDatatype,
    f: impl Fn(*mut InfoDatatype, *mut c_void, *mut usize) -> GcError,
) -> [u8; N] {
    let (ty, bytes) =
        query(f).unwrap_or_else(|rc| panic!("query failed: {rc} ({:?})", last_error()));
    assert_eq!(ty, expected, "datatype");
    bytes.as_slice().try_into().expect("size of the datatype")
}

#[track_caller]
pub(crate) fn query_u32(f: impl Fn(*mut InfoDatatype, *mut c_void, *mut usize) -> GcError) -> u32 {
    u32::from_ne_bytes(query_fixed(INFO_DATATYPE_UINT32, f))
}

#[track_caller]
pub(crate) fn query_i32(f: impl Fn(*mut InfoDatatype, *mut c_void, *mut usize) -> GcError) -> i32 {
    i32::from_ne_bytes(query_fixed(INFO_DATATYPE_INT32, f))
}

#[track_caller]
pub(crate) fn query_u64(f: impl Fn(*mut InfoDatatype, *mut c_void, *mut usize) -> GcError) -> u64 {
    u64::from_ne_bytes(query_fixed(INFO_DATATYPE_UINT64, f))
}

#[track_caller]
pub(crate) fn query_size(
    f: impl Fn(*mut InfoDatatype, *mut c_void, *mut usize) -> GcError,
) -> usize {
    usize::from_ne_bytes(query_fixed(INFO_DATATYPE_SIZET, f))
}

#[track_caller]
pub(crate) fn query_ptr(
    f: impl Fn(*mut InfoDatatype, *mut c_void, *mut usize) -> GcError,
) -> usize {
    usize::from_ne_bytes(query_fixed(INFO_DATATYPE_PTR, f))
}

#[track_caller]
pub(crate) fn query_bool(
    f: impl Fn(*mut InfoDatatype, *mut c_void, *mut usize) -> GcError,
) -> bool {
    query_fixed::<1>(INFO_DATATYPE_BOOL8, f)[0] != 0
}

/// An ID from `TLGetInterfaceID`, `IFGetDeviceID` or `DevGetDataStreamID`.
#[track_caller]
pub(crate) fn id(f: impl Fn(*mut c_char, *mut usize) -> GcError) -> String {
    let mut size = 0usize;
    check(f(null_mut(), &mut size));
    let mut text = vec![0u8; size];
    check(f(text.as_mut_ptr().cast(), &mut size));
    assert_eq!(size, text.len());
    c_text(&text)
}

/// `GCReadPort`.
pub(crate) fn read_port(port: Handle, address: u64, len: usize) -> Result<Vec<u8>, GcError> {
    let mut data = vec![0u8; len];
    let mut size = len;
    // SAFETY: `data` holds `len` bytes.
    let rc = unsafe { GCReadPort(port, address, data.as_mut_ptr().cast(), &mut size) };
    if rc != GC_ERR_SUCCESS {
        return Err(rc);
    }
    assert_eq!(size, len, "a successful read reads everything asked for");
    Ok(data)
}

/// `GCWritePort`.
pub(crate) fn write_port(port: Handle, address: u64, data: &[u8]) -> Result<(), GcError> {
    let mut size = data.len();
    // SAFETY: `data` holds `size` bytes.
    let rc = unsafe { GCWritePort(port, address, data.as_ptr().cast(), &mut size) };
    if rc != GC_ERR_SUCCESS {
        return Err(rc);
    }
    assert_eq!(size, data.len(), "a successful write writes everything");
    Ok(())
}

/// The first URL of a port, from `GCGetPortURLInfo`.
#[track_caller]
pub(crate) fn port_url(port: Handle) -> String {
    let mut count = 0u32;
    // SAFETY: valid pointer.
    check(unsafe { GCGetNumPortURLs(port, &mut count) });
    assert!(count >= 1, "a port serves at least one URL");
    query_string(|t, b, s| unsafe { GCGetPortURLInfo(port, 0, URL_INFO_URL, t, b, s) })
}

/// A `local:` URL's address and length (GenTL §4.1.2.1, Table 4-1).
#[track_caller]
pub(crate) fn local_url(url: &str) -> (String, u64, usize) {
    let (scheme, rest) = url.split_once(':').expect("scheme");
    assert!(scheme.eq_ignore_ascii_case("local"), "{url}");
    let rest = rest.trim_start_matches('/');
    let rest = rest.split('?').next().expect("path");
    let parts: Vec<&str> = rest.split(';').collect();
    assert_eq!(parts.len(), 3, "{url}");
    (
        parts[0].to_string(),
        u64::from_str_radix(parts[1], 16).expect("hex address"),
        usize::from_str_radix(parts[2], 16).expect("hex length"),
    )
}

/// The GenApi XML a port serves, read the way §4.1.3's example does.
#[track_caller]
pub(crate) fn port_xml(port: Handle) -> String {
    let url = port_url(port);
    let (_, address, length) = local_url(&url);
    let bytes = read_port(port, address, length)
        .unwrap_or_else(|rc| panic!("reading {url}: {rc} ({:?})", last_error()));
    String::from_utf8(bytes).expect("the XML is UTF-8")
}

/// A GenTL port as a [`RegisterIo`](viva_genicam::genapi::RegisterIo), so our
/// own GenApi engine can evaluate a module's XML against the producer.
pub(crate) struct PortIo(pub(crate) Handle);

impl viva_genicam::genapi::RegisterIo for PortIo {
    fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, viva_genicam::genapi::GenApiError> {
        read_port(self.0, addr, len)
            .map_err(|rc| viva_genicam::genapi::GenApiError::io(format!("GCReadPort: {rc}")))
    }

    fn write(&self, addr: u64, data: &[u8]) -> Result<(), viva_genicam::genapi::GenApiError> {
        write_port(self.0, addr, data)
            .map_err(|rc| viva_genicam::genapi::GenApiError::io(format!("GCWritePort: {rc}")))
    }
}

/// Build our own node map from the XML a port serves.
#[track_caller]
pub(crate) fn port_nodemap(port: Handle) -> viva_genicam::genapi::NodeMap {
    let xml = port_xml(port);
    let model = viva_genapi_xml::parse(&xml).expect("the module XML parses");
    viva_genicam::genapi::NodeMap::try_from_xml(model).expect("the module XML builds a node map")
}

/// A fake camera on 127.0.0.1:3956, and the runtime it runs on.
pub(crate) struct Fake {
    runtime: tokio::runtime::Runtime,
    camera: Option<FakeCamera>,
}

/// The fake's image, for tests that check pixels: Mono16 is sent as the
/// byte ramp `(i * 7 + block id) mod 256`, exact integer arithmetic a test
/// can recompute (`viva-fake-gige`'s `generate_test_pattern`).
pub(crate) const WIDTH: u32 = 64;
pub(crate) const HEIGHT: u32 = 48;
pub(crate) const MONO16: u32 = viva_fake_gige::MONO16;

impl Fake {
    pub(crate) fn start() -> Self {
        Self::start_with(|builder| builder)
    }

    pub(crate) fn start_with(customize: impl Fn(FakeCameraBuilder) -> FakeCameraBuilder) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("runtime");
        let builder = || {
            customize(
                FakeCamera::builder()
                    .width(WIDTH)
                    .height(HEIGHT)
                    .fps(30)
                    .pixel_format(MONO16)
                    .bind_ip([127, 0, 0, 1].into())
                    .port(3956),
            )
        };
        let camera = runtime.block_on(async {
            // The port the previous test's camera held may take a moment to free.
            let mut attempt = 0;
            loop {
                match builder().build().await {
                    Ok(camera) => break camera,
                    Err(_) if attempt < 20 => {
                        attempt += 1;
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    Err(err) => panic!("fake camera: {err}"),
                }
            }
        });
        Fake {
            runtime,
            camera: Some(camera),
        }
    }

    pub(crate) fn camera(&self) -> &FakeCamera {
        self.camera.as_ref().expect("running")
    }

    /// Read the fake's state directly, past GVCP.
    pub(crate) fn peek(&self, addr: u64, len: usize) -> Vec<u8> {
        self.runtime.block_on(self.camera().peek(addr, len))
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        if let Some(camera) = self.camera.take() {
            self.runtime.block_on(camera.stop());
        }
    }
}

/// The handles of a camera opened through the producer.
pub(crate) struct Opened {
    pub(crate) system: Handle,
    pub(crate) interface: Handle,
    pub(crate) device: Handle,
}

/// Open the system, the loopback interface (by its address, a convenience
/// name §3.2 allows) and the fake camera on it, as a consumer would.
#[track_caller]
pub(crate) fn open_fake(access: DeviceAccessFlags) -> Opened {
    let mut system = GENTL_INVALID_HANDLE;
    // SAFETY: valid pointers throughout.
    unsafe {
        check(TLOpen(&mut system));
        check(TLUpdateInterfaceList(system, null_mut(), 1000));
        let mut interface = GENTL_INVALID_HANDLE;
        let loopback = cstring("127.0.0.1");
        check(TLOpenInterface(system, loopback.as_ptr(), &mut interface));
        check(IFUpdateDeviceList(interface, null_mut(), 500));
        let device_id = fake_device_id(interface);
        let mut device = GENTL_INVALID_HANDLE;
        let id = cstring(&device_id);
        check(IFOpenDevice(interface, id.as_ptr(), access, &mut device));
        Opened {
            system,
            interface,
            device,
        }
    }
}

/// The ID of the fake camera in an interface's device list.
#[track_caller]
pub(crate) fn fake_device_id(interface: Handle) -> String {
    let mut count = 0u32;
    // SAFETY: valid pointers.
    check(unsafe { IFGetNumDevices(interface, &mut count) });
    for index in 0..count {
        let device_id = id(|b, s| unsafe { IFGetDeviceID(interface, index, b, s) });
        let id_c = cstring(&device_id);
        let model = query_string(|t, b, s| unsafe {
            IFGetDeviceInfo(interface, id_c.as_ptr(), DEVICE_INFO_MODEL, t, b, s)
        });
        if model == viva_fake_gige::FAKE_MODEL {
            return device_id;
        }
    }
    panic!("the fake camera is not among the {count} devices on loopback");
}

/// The remote device port of an opened device.
#[track_caller]
pub(crate) fn remote_port(device: Handle) -> Handle {
    let mut port = GENTL_INVALID_HANDLE;
    // SAFETY: valid pointer.
    check(unsafe { DevGetPort(device, &mut port) });
    port
}

/// Open the device's first data stream.
#[track_caller]
pub(crate) fn open_stream(device: Handle) -> Handle {
    let stream_id = id(|b, s| unsafe { DevGetDataStreamID(device, 0, b, s) });
    let id_c = cstring(&stream_id);
    let mut stream = GENTL_INVALID_HANDLE;
    // SAFETY: valid pointers.
    check(unsafe { DevOpenDataStream(device, id_c.as_ptr(), &mut stream) });
    stream
}

/// Execute the fake's `AcquisitionStart` / `AcquisitionStop` through the
/// remote port, as a consumer's GenApi would.
#[track_caller]
pub(crate) fn acquisition(remote: Handle, start: bool) {
    let register = if start {
        viva_fake_gige::REG_ACQ_START
    } else {
        viva_fake_gige::REG_ACQ_STOP
    };
    write_port(remote, register, &1u32.to_be_bytes())
        .unwrap_or_else(|rc| panic!("acquisition command: {rc} ({:?})", last_error()));
}

/// The byte the fake sends at `index` of Mono16 block `block_id`.
pub(crate) fn ramp(index: usize, block_id: u64) -> u8 {
    (index as u32)
        .wrapping_mul(7)
        .wrapping_add(block_id as u16 as u32) as u8
}

/// `EVENT_NEW_BUFFER_DATA` from `EventGetData`, or its error.
pub(crate) fn next_buffer(event: Handle, timeout_ms: u64) -> Result<EventNewBufferData, GcError> {
    let mut data = EventNewBufferData {
        buffer_handle: null_mut(),
        user_pointer: null_mut(),
    };
    let mut size = std::mem::size_of::<EventNewBufferData>();
    // SAFETY: `data` holds `size` bytes.
    let rc = unsafe {
        EventGetData(
            event,
            (&mut data as *mut EventNewBufferData).cast(),
            &mut size,
            timeout_ms,
        )
    };
    if rc != GC_ERR_SUCCESS {
        return Err(rc);
    }
    assert_eq!(size, std::mem::size_of::<EventNewBufferData>());
    Ok(data)
}

/// A buffer info query on `buffer`.
pub(crate) fn buffer_info(
    stream: Handle,
    buffer: Handle,
    cmd: BufferInfoCmd,
) -> impl Fn(*mut InfoDatatype, *mut c_void, *mut usize) -> GcError {
    move |t, b, s| unsafe { DSGetBufferInfo(stream, buffer, cmd, t, b, s) }
}
