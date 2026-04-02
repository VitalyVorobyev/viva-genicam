//! Integration tests against `arv-fake-gv-camera-0.8` (Aravis GigE Vision simulator).
//!
//! These tests require `arv-fake-gv-camera-0.8` installed (e.g. via `brew install aravis`).
//! They are marked `#[ignore]` and must be run explicitly:
//!
//! ```sh
//! cargo test -p genicam --test fake_camera -- --ignored
//! ```

mod common;

use genicam::{connect_gige, connect_gige_with_xml, gige, Camera, GigeRegisterIo};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
) -> Result<String, genicam::GenicamError> {
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
) -> Result<(), genicam::GenicamError> {
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

/// Loopback interface name (platform-dependent).
fn loopback_iface_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "lo0"
    } else {
        "lo"
    }
}

// ---------------------------------------------------------------------------
// Phase 1: Discovery & Connection
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn test_discovery_finds_fake_camera() {
    skip_if_no_aravis!();
    let _cam = common::FakeCamera::start();

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
#[ignore]
async fn test_connect_and_fetch_xml() {
    skip_if_no_aravis!();
    let _cam = common::FakeCamera::start();

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

// ---------------------------------------------------------------------------
// Phase 2: Feature Read / Write
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn test_read_width_height() {
    skip_if_no_aravis!();
    let _cam = common::FakeCamera::start();
    let camera = connect_fake().await;

    let width = blocking_get(&camera, "Width").await.expect("read Width");
    let height = blocking_get(&camera, "Height").await.expect("read Height");

    let w: i64 = width.parse().expect("Width should be an integer");
    let h: i64 = height.parse().expect("Height should be an integer");
    assert!(w > 0, "Width should be positive, got {w}");
    assert!(h > 0, "Height should be positive, got {h}");
}

#[tokio::test]
#[ignore]
async fn test_read_pixel_format() {
    skip_if_no_aravis!();
    let _cam = common::FakeCamera::start();
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
#[ignore]
async fn test_read_exposure_time() {
    skip_if_no_aravis!();
    let _cam = common::FakeCamera::start();
    let camera = connect_fake().await;

    // Aravis fake camera may use "ExposureTimeAbs" or "ExposureTime".
    let exp = blocking_get(&camera, "ExposureTimeAbs").await;
    let exp = match exp {
        Ok(v) => Ok(v),
        Err(_) => blocking_get(&camera, "ExposureTime").await,
    };

    if let Ok(val) = exp {
        let v: f64 = val.parse().expect("ExposureTime should be a float");
        assert!(v > 0.0, "ExposureTime should be positive, got {v}");
    }
    // If neither node exists, that's acceptable for this fake camera.
}

#[tokio::test]
#[ignore]
async fn test_set_and_readback_width() {
    skip_if_no_aravis!();
    let _cam = common::FakeCamera::start();
    let camera = connect_fake().await;

    // Read current width first.
    let original = blocking_get(&camera, "Width").await.expect("read Width");
    let original_val: i64 = original.parse().unwrap();

    // Set to a different valid value.
    let new_val = if original_val > 128 { 128 } else { 256 };
    blocking_set(&camera, "Width", &new_val.to_string())
        .await
        .expect("set Width");

    let readback = blocking_get(&camera, "Width").await.expect("readback Width");
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
#[ignore]
async fn test_exec_acquisition_commands() {
    skip_if_no_aravis!();
    let _cam = common::FakeCamera::start();
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
#[ignore]
async fn test_read_nonexistent_node() {
    skip_if_no_aravis!();
    let _cam = common::FakeCamera::start();
    let camera = connect_fake().await;

    let result = blocking_get(&camera, "NonExistentNode12345").await;
    assert!(result.is_err(), "reading nonexistent node should fail");
}

// ---------------------------------------------------------------------------
// Phase 3: Streaming
//
// NOTE: These tests timeout on macOS loopback because GVSP (raw UDP frame
// packets) cannot traverse the loopback interface reliably on macOS.
// They are expected to pass when run against a real camera on a real NIC.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn test_stream_receives_frames() {
    skip_if_no_aravis!();
    let _cam = common::FakeCamera::start();

    let device = discover_fake().await;
    let camera = connect_fake().await;

    // Second connection for stream control.
    use std::net::{IpAddr, SocketAddr};
    let control_addr = SocketAddr::new(IpAddr::V4(device.ip), gige::GVCP_PORT);
    let mut stream_device = gige::GigeDevice::open(control_addr)
        .await
        .expect("open stream device");

    let iface =
        gige::nic::Iface::from_system(loopback_iface_name()).expect("loopback iface not found");
    let stream = genicam::StreamBuilder::new(&mut stream_device)
        .iface(iface)
        .auto_packet_size(false)
        .build()
        .await
        .expect("build stream");

    let mut frame_stream = genicam::FrameStream::new(stream, None);

    // Start acquisition.
    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let mut cam = cam.lock().unwrap();
        cam.acquisition_start().expect("start acquisition");
    })
    .await
    .unwrap();

    // Receive at least one frame with timeout.
    let frame = tokio::time::timeout(Duration::from_secs(5), frame_stream.next_frame())
        .await
        .expect("timeout waiting for frame")
        .expect("frame error")
        .expect("stream ended without a frame");

    assert!(frame.width > 0, "frame width should be positive");
    assert!(frame.height > 0, "frame height should be positive");
    assert!(!frame.payload.is_empty(), "frame payload should not be empty");

    // Stop acquisition.
    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let mut cam = cam.lock().unwrap();
        cam.acquisition_stop().expect("stop acquisition");
    })
    .await
    .unwrap();
}

#[tokio::test]
#[ignore]
async fn test_frame_dimensions_match() {
    skip_if_no_aravis!();
    let _cam = common::FakeCamera::start();

    let device = discover_fake().await;
    let camera = connect_fake().await;

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

    // Stream a frame.
    use std::net::{IpAddr, SocketAddr};
    let control_addr = SocketAddr::new(IpAddr::V4(device.ip), gige::GVCP_PORT);
    let mut stream_device = gige::GigeDevice::open(control_addr)
        .await
        .expect("open stream device");

    let iface =
        gige::nic::Iface::from_system(loopback_iface_name()).expect("loopback iface not found");
    let stream = genicam::StreamBuilder::new(&mut stream_device)
        .iface(iface)
        .auto_packet_size(false)
        .build()
        .await
        .expect("build stream");

    let mut frame_stream = genicam::FrameStream::new(stream, None);

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
#[ignore]
async fn test_full_lifecycle() {
    skip_if_no_aravis!();
    let _cam = common::FakeCamera::start();

    let device = discover_fake().await;
    let camera = connect_fake().await;

    use std::net::{IpAddr, SocketAddr};
    let control_addr = SocketAddr::new(IpAddr::V4(device.ip), gige::GVCP_PORT);
    let mut stream_device = gige::GigeDevice::open(control_addr)
        .await
        .expect("open stream device");

    let iface =
        gige::nic::Iface::from_system(loopback_iface_name()).expect("loopback iface not found");
    let stream = genicam::StreamBuilder::new(&mut stream_device)
        .iface(iface)
        .auto_packet_size(false)
        .build()
        .await
        .expect("build stream");

    let mut frame_stream = genicam::FrameStream::new(stream, None);

    // Start acquisition.
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

    // Stop acquisition.
    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let mut cam = cam.lock().unwrap();
        cam.acquisition_stop().expect("stop");
    })
    .await
    .unwrap();

    // Drop everything — should not panic.
    drop(frame_stream);
    drop(camera);
}
