//! Integration tests using the in-process fake GigE Vision camera.
//!
//! These tests require no external dependencies — `fake-gige` provides a
//! self-contained GVCP/GVSP camera on localhost.
//!
//! ```sh
//! cargo test -p genicam --test fake_camera
//! ```

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;
#[allow(clippy::single_component_path_imports)]
use viva_genapi_xml;
use viva_genicam::{Camera, GigeRegisterIo, connect_gige, connect_gige_with_xml, gige};

/// Helper: discover the fake camera via loopback.
async fn discover_fake() -> gige::DeviceInfo {
    let devices = gige::discover_all(Duration::from_secs(2))
        .await
        .expect("discovery failed");
    devices
        .into_iter()
        .find(|d| d.ip.is_loopback())
        .expect("fake camera not found on loopback")
}

/// Helper: connect to the fake camera, returning a shared handle safe for spawn_blocking.
async fn connect_fake() -> Arc<Mutex<Camera<GigeRegisterIo>>> {
    let device = discover_fake().await;
    let camera = connect_gige(&device).await.expect("connect failed");
    Arc::new(Mutex::new(camera))
}

/// Run a blocking camera operation from an async context.
async fn blocking_get(
    camera: &Arc<Mutex<Camera<GigeRegisterIo>>>,
    name: &str,
) -> Result<String, viva_genicam::GenicamError> {
    let cam = camera.clone();
    let name = name.to_string();
    tokio::task::spawn_blocking(move || {
        let cam = cam.lock().unwrap();
        cam.get(&name)
    })
    .await
    .unwrap()
}

async fn blocking_set(
    camera: &Arc<Mutex<Camera<GigeRegisterIo>>>,
    name: &str,
    value: &str,
) -> Result<(), viva_genicam::GenicamError> {
    let cam = camera.clone();
    let name = name.to_string();
    let value = value.to_string();
    tokio::task::spawn_blocking(move || {
        let mut cam = cam.lock().unwrap();
        cam.set(&name, &value)
    })
    .await
    .unwrap()
}

/// Resolve the loopback network interface (platform-independent).
fn loopback_iface() -> gige::nic::Iface {
    gige::nic::Iface::from_ipv4(std::net::Ipv4Addr::LOCALHOST).expect("loopback iface")
}

// ---------------------------------------------------------------------------
// Phase 1: Discovery & Connection
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_discovery_finds_fake_camera() {
    let _cam = common::TestCamera::start().await;

    let devices = gige::discover_all(Duration::from_secs(2))
        .await
        .expect("discovery failed");

    assert!(!devices.is_empty(), "expected at least one device");
    let fake = devices
        .iter()
        .find(|d| d.ip.is_loopback())
        .expect("no loopback device found");
    assert!(
        fake.model.is_some() || fake.manufacturer.is_some(),
        "expected device identity fields"
    );
}

#[tokio::test]
async fn test_connect_and_fetch_xml() {
    let _cam = common::TestCamera::start().await;

    let device = discover_fake().await;
    let (camera, xml) = connect_gige_with_xml(&device)
        .await
        .expect("connect failed");

    // XML should be non-empty and look like GenICam XML.
    assert!(!xml.is_empty(), "XML should not be empty");
    assert!(
        xml.contains("RegisterDescription") || xml.contains("Category"),
        "XML should contain GenICam elements"
    );

    // NodeMap should contain standard SFNC nodes.
    let nodemap = camera.nodemap();
    assert!(
        nodemap.node("Width").is_some(),
        "NodeMap should contain Width"
    );
    assert!(
        nodemap.node("Height").is_some(),
        "NodeMap should contain Height"
    );
}

#[tokio::test]
async fn test_claim_control_visible_via_register_read() {
    let _cam = common::TestCamera::start().await;

    let device_info = discover_fake().await;
    use std::net::{IpAddr, SocketAddr};

    let control_addr = SocketAddr::new(IpAddr::V4(device_info.ip), gige::GVCP_PORT);
    let mut device = gige::GigeDevice::open(control_addr)
        .await
        .expect("open device");

    device.claim_control().await.expect("claim CCP");

    let privilege = device
        .read_register(gige::gvcp::consts::CONTROL_CHANNEL_PRIVILEGE as u32)
        .await
        .expect("read CCP register");
    let controller_bits = gige::gvcp::consts::CCP_CONTROL | gige::gvcp::consts::CCP_EXCLUSIVE;
    assert_ne!(
        privilege & controller_bits,
        0,
        "CCP register should report an active controller, got 0x{privilege:08x}"
    );

    device.release_control().await.expect("release CCP");
}

// ---------------------------------------------------------------------------
// Phase 2: Feature Read / Write
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_read_width_height() {
    let _cam = common::TestCamera::start().await;
    let camera = connect_fake().await;

    let width = blocking_get(&camera, "Width").await.expect("read Width");
    let height = blocking_get(&camera, "Height").await.expect("read Height");

    let w: i64 = width.parse().expect("Width should be an integer");
    let h: i64 = height.parse().expect("Height should be an integer");
    assert!(w > 0, "Width should be positive, got {w}");
    assert!(h > 0, "Height should be positive, got {h}");
}

#[tokio::test]
async fn test_read_pixel_format() {
    let _cam = common::TestCamera::start().await;
    let camera = connect_fake().await;

    let pf = blocking_get(&camera, "PixelFormat")
        .await
        .expect("read PixelFormat");
    assert!(
        !pf.is_empty(),
        "PixelFormat should return a non-empty string"
    );
}

#[tokio::test]
async fn test_read_exposure_time() {
    let _cam = common::TestCamera::start().await;
    let camera = connect_fake().await;

    let exp = blocking_get(&camera, "ExposureTime")
        .await
        .expect("read ExposureTime");

    let v: f64 = exp.parse().expect("ExposureTime should be a float");
    assert!(v > 0.0, "ExposureTime should be positive, got {v}");
}

#[tokio::test]
async fn test_set_and_readback_width() {
    let _cam = common::TestCamera::start().await;
    let camera = connect_fake().await;

    // Read current width first.
    let original = blocking_get(&camera, "Width").await.expect("read Width");
    let original_val: i64 = original.parse().unwrap();

    // Set to a different valid value.
    let new_val = if original_val > 128 { 128 } else { 256 };
    blocking_set(&camera, "Width", &new_val.to_string())
        .await
        .expect("set Width");

    let readback = blocking_get(&camera, "Width")
        .await
        .expect("readback Width");
    let readback_val: i64 = readback.parse().unwrap();
    assert_eq!(
        readback_val, new_val,
        "Width readback should match set value"
    );

    // Restore original.
    blocking_set(&camera, "Width", &original)
        .await
        .expect("restore Width");
}

#[tokio::test]
async fn test_exec_acquisition_commands() {
    let _cam = common::TestCamera::start().await;
    let camera = connect_fake().await;

    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let mut cam = cam.lock().unwrap();
        cam.acquisition_start().expect("acquisition_start");
        cam.acquisition_stop().expect("acquisition_stop");
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn test_read_nonexistent_node() {
    let _cam = common::TestCamera::start().await;
    let camera = connect_fake().await;

    let result = blocking_get(&camera, "NonExistentNode12345").await;
    assert!(result.is_err(), "reading nonexistent node should fail");
}

// ---------------------------------------------------------------------------
// Phase 3: Streaming
// ---------------------------------------------------------------------------

/// Helper: set up a streaming session.
async fn setup_stream(
    device_info: &gige::DeviceInfo,
) -> (
    viva_genicam::FrameStream,
    Arc<Mutex<Camera<GigeRegisterIo>>>,
) {
    use std::net::{IpAddr, SocketAddr};

    let control_addr = SocketAddr::new(IpAddr::V4(device_info.ip), gige::GVCP_PORT);

    // Fetch XML via a temporary connection.
    let xml = viva_genapi_xml::fetch_and_load_xml({
        let addr = control_addr;
        move |address, length| {
            let addr = addr;
            async move {
                let mut dev = gige::GigeDevice::open(addr)
                    .await
                    .map_err(|e| viva_genapi_xml::XmlError::Transport(e.to_string()))?;
                dev.read_mem(address, length)
                    .await
                    .map_err(|e| viva_genapi_xml::XmlError::Transport(e.to_string()))
            }
        }
    })
    .await
    .expect("fetch XML");

    let model = viva_genapi_xml::parse(&xml).expect("parse XML");
    let nodemap = viva_genicam::genapi::NodeMap::from(model);

    // Main device: claim CCP, configure stream.
    let mut device = gige::GigeDevice::open(control_addr)
        .await
        .expect("open device");
    device.claim_control().await.expect("claim control");

    let iface = loopback_iface();
    let stream = viva_genicam::StreamBuilder::new(&mut device)
        .iface(iface)
        .auto_packet_size(false)
        .build()
        .await
        .expect("build stream");
    let frame_stream = viva_genicam::FrameStream::new(stream, None);

    let handle = tokio::runtime::Handle::current();
    let transport = GigeRegisterIo::new(handle, device);
    let camera = Arc::new(Mutex::new(Camera::new(transport, nodemap)));

    (frame_stream, camera)
}

#[tokio::test]
async fn test_stream_receives_frames() {
    let _cam = common::TestCamera::start().await;
    let device_info = discover_fake().await;

    let (mut frame_stream, camera) = setup_stream(&device_info).await;

    // Start acquisition.
    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let mut cam = cam.lock().unwrap();
        cam.acquisition_start().expect("start acquisition");
    })
    .await
    .unwrap();

    // Receive at least one frame.
    let frame = tokio::time::timeout(Duration::from_secs(5), frame_stream.next_frame())
        .await
        .expect("timeout waiting for frame")
        .expect("frame error")
        .expect("stream ended without a frame");

    assert!(frame.width > 0, "frame width should be positive");
    assert!(frame.height > 0, "frame height should be positive");
    assert!(
        !frame.payload.is_empty(),
        "frame payload should not be empty"
    );

    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let mut cam = cam.lock().unwrap();
        cam.acquisition_stop().expect("stop acquisition");
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn test_frame_dimensions_match() {
    let _cam = common::TestCamera::start().await;
    let device_info = discover_fake().await;

    let (mut frame_stream, camera) = setup_stream(&device_info).await;

    // Read expected dimensions.
    let expected_w: u32 = blocking_get(&camera, "Width")
        .await
        .expect("Width")
        .parse()
        .unwrap();
    let expected_h: u32 = blocking_get(&camera, "Height")
        .await
        .expect("Height")
        .parse()
        .unwrap();

    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let mut cam = cam.lock().unwrap();
        cam.acquisition_start().expect("start");
    })
    .await
    .unwrap();

    let frame = tokio::time::timeout(Duration::from_secs(5), frame_stream.next_frame())
        .await
        .expect("timeout")
        .expect("frame error")
        .expect("stream ended without a frame");

    assert_eq!(frame.width, expected_w, "frame width mismatch");
    assert_eq!(frame.height, expected_h, "frame height mismatch");

    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let mut cam = cam.lock().unwrap();
        cam.acquisition_stop().expect("stop");
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn test_full_lifecycle() {
    let _cam = common::TestCamera::start().await;
    let device_info = discover_fake().await;

    let (mut frame_stream, camera) = setup_stream(&device_info).await;

    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let mut cam = cam.lock().unwrap();
        cam.acquisition_start().expect("start");
    })
    .await
    .unwrap();

    // Receive 5 frames.
    for i in 0..5 {
        let frame = tokio::time::timeout(Duration::from_secs(5), frame_stream.next_frame())
            .await
            .unwrap_or_else(|_| panic!("timeout on frame {i}"))
            .unwrap_or_else(|e| panic!("error on frame {i}: {e}"))
            .unwrap_or_else(|| panic!("stream ended on frame {i}"));
        assert!(frame.width > 0);
        assert!(frame.height > 0);
    }

    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let mut cam = cam.lock().unwrap();
        cam.acquisition_stop().expect("stop");
    })
    .await
    .unwrap();

    drop(frame_stream);
    drop(camera);
}

// ---------------------------------------------------------------------------
// Phase 4: IP management
// ---------------------------------------------------------------------------

/// Helper: open a `GigeDevice` control connection to the fake camera.
async fn open_fake_device(device_info: &gige::DeviceInfo) -> gige::GigeDevice {
    use std::net::{IpAddr, SocketAddr};
    let addr = SocketAddr::new(IpAddr::V4(device_info.ip), gige::GVCP_PORT);
    gige::GigeDevice::open(addr).await.expect("open GigeDevice")
}

#[tokio::test]
async fn test_persistent_ip_roundtrip() {
    let _cam = common::TestCamera::start().await;
    let device_info = discover_fake().await;

    let mut device = open_fake_device(&device_info).await;
    device.claim_control().await.expect("claim control");

    let ip: std::net::Ipv4Addr = "192.168.10.50".parse().unwrap();
    let subnet: std::net::Ipv4Addr = "255.255.255.0".parse().unwrap();
    let gateway: std::net::Ipv4Addr = "192.168.10.1".parse().unwrap();

    device
        .write_persistent_ip(ip, subnet, gateway)
        .await
        .expect("write_persistent_ip");

    let (read_ip, read_subnet, read_gateway) = device
        .read_persistent_ip()
        .await
        .expect("read_persistent_ip");

    assert_eq!(read_ip, ip, "persistent IP roundtrip mismatch");
    assert_eq!(read_subnet, subnet, "persistent subnet roundtrip mismatch");
    assert_eq!(
        read_gateway, gateway,
        "persistent gateway roundtrip mismatch"
    );

    device
        .enable_persistent_ip()
        .await
        .expect("enable_persistent_ip");

    device.release_control().await.expect("release control");
}

/// Test FORCEIP against the fake GigE camera.
///
/// On macOS, UDP broadcast from a loopback-bound socket is rejected by the OS
/// ("Can't assign requested address"), so this test is skipped on macOS.
/// It runs on Linux where loopback broadcast is supported.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn test_force_ip() {
    let _cam = common::TestCamera::start().await;
    let device_info = discover_fake().await;

    // The fake camera MAC is DE:AD:BE:EF:CA:FE (hard-coded in fake-gige).
    let mac: [u8; 6] = [0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE];
    let ip: std::net::Ipv4Addr = "192.168.1.100".parse().unwrap();
    let subnet: std::net::Ipv4Addr = "255.255.255.0".parse().unwrap();
    let gateway: std::net::Ipv4Addr = "192.168.1.1".parse().unwrap();

    // Use the loopback interface so the broadcast reaches the fake camera.
    let iface = loopback_iface();

    // The fake camera accepts FORCEIP for its MAC and sends a FORCEIP_ACK.
    gige::force_ip(mac, ip, subnet, gateway, Some(&iface))
        .await
        .expect("force_ip should succeed against fake camera");

    // The fake camera is still reachable afterwards (it ignores the IP change).
    let device_info2 = discover_fake().await;
    assert_eq!(
        device_info.ip, device_info2.ip,
        "fake camera IP should not change after FORCEIP"
    );
}
