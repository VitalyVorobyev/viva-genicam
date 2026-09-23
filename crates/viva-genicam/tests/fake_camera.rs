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

/// Read raw register bytes through the camera's own transport, bypassing the
/// nodemap. Used to pin what the device actually stores before asserting what
/// the codec makes of it.
async fn blocking_read_register(
    camera: &Arc<Mutex<Camera<GigeRegisterIo>>>,
    address: u64,
    len: usize,
) -> Vec<u8> {
    use viva_genicam::genapi::RegisterIo;
    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let cam = cam.lock().unwrap();
        cam.transport().read(address, len).expect("raw read")
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

    // Assert the values, not merely that some field is populated. The previous
    // `model.is_some() || manufacturer.is_some()` disjunction passed happily
    // while the MAC was being read two bytes off (#57), and would have passed
    // with every string field empty. Per ADR-0019, identity assertions check
    // exactly what the fake was configured to report.
    assert_eq!(fake.mac, viva_fake_gige::FAKE_MAC, "MAC address mismatch");
    assert_eq!(
        fake.manufacturer.as_deref(),
        Some(viva_fake_gige::FAKE_MANUFACTURER)
    );
    assert_eq!(fake.model.as_deref(), Some(viva_fake_gige::FAKE_MODEL));
    assert_eq!(fake.version.as_deref(), Some(viva_fake_gige::FAKE_VERSION));
    assert_eq!(fake.serial.as_deref(), Some(viva_fake_gige::FAKE_SERIAL));
    assert_eq!(
        fake.user_name.as_deref(),
        Some(viva_fake_gige::FAKE_USER_NAME)
    );
}

/// Check the fake's Discovery ACK **bytes** against the specification's field
/// table, without going through `parse_discovery_payload`.
///
/// This is the ADR-0019 rule in practice. Routing the fake's output through our
/// own parser only proves the two agree with each other — which they did for
/// three separate wire bugs (the SCPS overhead, unaligned READMEM, and the MAC
/// offset in #57), each time while both disagreed with the standard.
#[tokio::test]
async fn test_fake_discovery_ack_matches_spec_layout() {
    use tokio::net::UdpSocket;

    let _cam = common::TestCamera::start().await;

    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    let request_id: u16 = 0x0142;

    // GVCP command: 0x42 key, flags, opcode, length, request id.
    let mut cmd = Vec::new();
    cmd.push(0x42);
    cmd.push(0x11); // ACK_REQUIRED | BROADCAST
    cmd.extend_from_slice(&0x0002u16.to_be_bytes()); // DISCOVERY_CMD
    cmd.extend_from_slice(&0u16.to_be_bytes()); // no payload
    cmd.extend_from_slice(&request_id.to_be_bytes());
    socket
        .send_to(&cmd, ("127.0.0.1", gige::GVCP_PORT))
        .await
        .expect("send discovery");

    let mut buf = vec![0u8; 1024];
    let (len, _) = tokio::time::timeout(Duration::from_secs(2), socket.recv_from(&mut buf))
        .await
        .expect("timed out waiting for discovery ack")
        .expect("recv");

    // 8-byte GVCP ack header.
    assert!(len >= 8 + 248, "ack shorter than a 248-byte payload: {len}");
    assert_eq!(u16::from_be_bytes([buf[0], buf[1]]), 0x0000, "status");
    assert_eq!(
        u16::from_be_bytes([buf[2], buf[3]]),
        0x0003,
        "DISCOVERY_ACK"
    );
    assert_eq!(u16::from_be_bytes([buf[4], buf[5]]), 248, "payload length");
    assert_eq!(u16::from_be_bytes([buf[6], buf[7]]), request_id);

    let p = &buf[8..8 + 248];
    let text = |at: usize, n: usize| {
        let field = &p[at..at + n];
        let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
        String::from_utf8_lossy(&field[..end]).to_string()
    };

    // Offsets from the specification's field table, corroborated by
    // Wireshark's dissect_discovery_ack().
    assert_eq!(&p[10..16], &viva_fake_gige::FAKE_MAC, "MAC at offset 10");
    assert_eq!(&p[36..40], &[127, 0, 0, 1], "current IP at offset 36");
    assert_eq!(text(72, 32), viva_fake_gige::FAKE_MANUFACTURER, "offset 72");
    assert_eq!(text(104, 32), viva_fake_gige::FAKE_MODEL, "offset 104");
    assert_eq!(text(136, 32), viva_fake_gige::FAKE_VERSION, "offset 136");
    assert_eq!(text(216, 16), viva_fake_gige::FAKE_SERIAL, "offset 216");
    assert_eq!(text(232, 16), viva_fake_gige::FAKE_USER_NAME, "offset 232");

    // Bytes 8..10 are the padding half of the MAC-high register. If a future
    // change reintroduces the old 4-byte skip, the MAC slides here.
    assert_eq!(&p[8..10], &[0, 0], "reserved MAC-high padding at offset 8");
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

/// A register whose address is `<Address>` + `<pIndex>` must resolve to the
/// sum, over the wire.
///
/// The GigE stream-channel block lives at `0x0D00 + channel * 0x40`, so
/// `GevSCPSPacketSize` is a fixed offset plus a scaled index. Dropping the
/// `<pIndex>` term would still return a plausible value — channel 0's — which
/// is why this needs an end-to-end check rather than a unit test.
#[tokio::test]
async fn test_summed_address_resolves_over_the_wire() {
    let _cam = common::TestCamera::start().await;
    let camera = connect_fake().await;

    // Channel 0 lives at 0x0D04, channel 1 at 0x0D44, and the fake camera gives
    // them different packet sizes. An ignored <pIndex> term would return
    // channel 0's value both times.
    let channel0 = blocking_get(&camera, "GevSCPSPacketSize")
        .await
        .expect("read channel 0 packet size");
    assert_eq!(channel0, "1500");

    blocking_set(&camera, "GevStreamChannelSelector", "1")
        .await
        .expect("select channel 1");
    let channel1 = blocking_get(&camera, "GevSCPSPacketSize")
        .await
        .expect("read channel 1 packet size");
    assert_eq!(
        channel1, "9000",
        "the <pIndex> term did not move the address"
    );
}

/// `<StructReg>` bits addressed through `<pAddress>` + `<Address>` read the
/// right register, and read as 1 rather than -1.
#[tokio::test]
async fn test_struct_reg_inquiry_bits_over_the_wire() {
    let _cam = common::TestCamera::start().await;
    let camera = connect_fake().await;

    // The fake camera reports 0xC0000000: capability bits 0 and 1 set, 2 clear.
    for (bit, expected) in [
        ("FrameRateControlInq_Bit", "1"),
        ("ChunkSupportInq_Bit", "1"),
        ("SequencerInq_Bit", "0"),
    ] {
        let value = blocking_get(&camera, bit).await.expect("read inquiry bit");
        assert_eq!(value, expected, "{bit} read as {value}");
    }
}

#[tokio::test]
async fn test_connect_with_zipped_xml() {
    // Many real cameras (Basler, FLIR, Hikrobot, ...) serve their GenApi XML
    // as a ZIP archive; issue #35 hit exactly this path. The archive length
    // is also virtually never 4-byte aligned, which exercises the READMEM
    // alignment handling against the strict fake GVCP server.
    let _cam = common::TestCamera::start_with(|builder| builder.zip_xml(true)).await;

    let device = discover_fake().await;
    let (camera, xml) = connect_gige_with_xml(&device)
        .await
        .expect("connect with zipped XML failed");

    assert!(
        xml.contains("RegisterDescription"),
        "decompressed XML should contain GenICam elements"
    );
    let nodemap = camera.nodemap();
    assert!(
        nodemap.node("Width").is_some(),
        "NodeMap should contain Width"
    );
}

#[tokio::test]
async fn test_connect_with_cdata_tooltip() {
    // Issue #45: a FLIR BFS-PGE camera could not be opened because a CDATA
    // section in a text element was run through XML unescaping, which chokes on
    // the literal `&` that is legal there. Connecting must succeed and the
    // tooltip must survive intact.
    let _cam = common::TestCamera::start().await;

    let device = discover_fake().await;
    let camera = connect_gige(&device).await.expect("connect failed");

    let tooltip = camera
        .nodemap()
        .node("Gain")
        .expect("NodeMap should contain Gain")
        .tooltip()
        .expect("Gain should have a tooltip");
    assert!(
        tooltip.contains("0 < gain & gain < 48"),
        "CDATA tooltip should be preserved verbatim, got: {tooltip}"
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
    // Fake camera seeds ExposureTime at 5000.0 µs as IEEE 754 f64.
    // Prior to the Float-encoding fix this came back as `4662219572839973000`
    // (the f64 bit pattern of 5000.0 interpreted as i64).
    assert!(
        (v - 5000.0).abs() < 1.0,
        "ExposureTime should be ≈ 5000.0 µs, got {v}"
    );
}

#[tokio::test]
async fn test_read_acquisition_frame_rate() {
    let _cam = common::TestCamera::start().await;
    let camera = connect_fake().await;

    let rate = blocking_get(&camera, "AcquisitionFrameRate")
        .await
        .expect("read AcquisitionFrameRate");

    let v: f64 = rate
        .parse()
        .expect("AcquisitionFrameRate should be a float");
    // Fake camera seeds AcquisitionFrameRate at 30.0 fps as IEEE 754 f32.
    // Prior to the Float-encoding fix this came back as `1106247680`.
    assert!(
        (v - 30.0).abs() < 1e-3,
        "AcquisitionFrameRate should be ≈ 30.0 fps, got {v}"
    );
}

#[tokio::test]
async fn test_exposure_time_roundtrip() {
    let _cam = common::TestCamera::start().await;
    let camera = connect_fake().await;

    blocking_set(&camera, "ExposureTime", "7500.0")
        .await
        .expect("set ExposureTime");

    let exp = blocking_get(&camera, "ExposureTime")
        .await
        .expect("read ExposureTime");
    let v: f64 = exp.parse().expect("ExposureTime should be a float");
    assert!((v - 7500.0).abs() < 1.0, "got {v}");

    blocking_set(&camera, "ExposureTime", "5000.0")
        .await
        .expect("restore ExposureTime");
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
    let (stream, camera) = setup_stream_owned(device_info).await;
    (stream, Arc::new(Mutex::new(camera)))
}

/// As [`setup_stream`], but hands back the `Camera` itself.
///
/// `configure_events` is `async` and takes `&mut self`, so a caller that needs
/// it cannot go through the `Arc<Mutex<_>>` wrapper — a std `MutexGuard` cannot
/// be held across an await.
async fn setup_stream_owned(
    device_info: &gige::DeviceInfo,
) -> (viva_genicam::FrameStream, Camera<GigeRegisterIo>) {
    setup_stream_sized(device_info, Some(1500)).await
}

/// As [`setup_stream_owned`], but leaves the packet size to the caller.
///
/// `None` means [`StreamBuilder::auto_packet_size`] (NIC MTU + SR-13 probe) —
/// the path SR-02 / SR-13 cover, where a camera can clamp or a path can bisect.
/// Pass `Some(n)` for an explicit ceiling. Preserve-by-default is tested
/// separately (ADR-0021 / SR-14).
async fn setup_stream_sized(
    device_info: &gige::DeviceInfo,
    packet_size: Option<u32>,
) -> (viva_genicam::FrameStream, Camera<GigeRegisterIo>) {
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
    let nodemap = viva_genicam::genapi::NodeMap::try_from_xml(model).expect("build nodemap");

    // Main device: claim CCP, configure stream.
    let mut device = gige::GigeDevice::open(control_addr)
        .await
        .expect("open device");
    device.claim_control().await.expect("claim control");

    let iface = loopback_iface();
    let mut builder = viva_genicam::StreamBuilder::new(&mut device).iface(iface);
    // Callers pin the packet size so reassembly is exercised over ~200 packets.
    // Loopback reports a jumbo MTU, which would reduce a 640x480 frame to a
    // handful of datagrams and stop testing the stride arithmetic that #34 got
    // wrong — unless they intentionally ask for Auto.
    if let Some(size) = packet_size {
        builder = builder.packet_size(size);
    } else {
        builder = builder.auto_packet_size();
    }
    let stream = builder.build().await.expect("build stream");
    let frame_stream = viva_genicam::FrameStream::new(stream, None);

    let handle = tokio::runtime::Handle::current();
    let transport = GigeRegisterIo::new(handle, device);

    (frame_stream, Camera::new(transport, nodemap))
}

/// A camera that clamps `GevSCPSPacketSize` must be followed, not assumed —
/// backlog SR-02, reported as
/// [#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112).
///
/// Loopback reports a jumbo MTU, so the builder asks for one; the fake caps the
/// register at 1 500 and acknowledges the write, exactly as the Vieworks
/// FS3200T in the report does. Before the read-back landed,
/// `StreamParams.packet_size` kept the *request*, `gvsp_payload_size` derived
/// every reassembly offset from it, and the stream carried packets while
/// completing no frame — the reporter's "0 frames on every Start".
///
/// This could not have been written before: the fake accepted any packet size,
/// so it could not express the camera that caused the report. That is the
/// ADR-0019 failure mode — a fake and a client agreeing only with each other —
/// and it is why `max_packet_size` exists.
///
/// The cap is deliberately **below** 1 500 rather than at it. `nic::mtu` probes
/// only on Linux and Windows and returns a hardcoded 1 500 elsewhere (TC-11),
/// so a 1 500-byte cap would leave request and cap equal on macOS and the test
/// would pass without ever exercising the clamp — verified by re-running it with
/// the read-back disabled, where a 1 500 cap still passed.
#[tokio::test]
async fn test_clamped_packet_size_is_followed_not_assumed() {
    const CAMERA_MAX: u32 = 1000;

    let _cam = common::TestCamera::start_with(|builder| builder.max_packet_size(CAMERA_MAX)).await;
    let device_info = discover_fake().await;

    // `None` = auto_packet_size (NIC MTU + probe), which on loopback is well
    // above the cap so the camera clamp is exercised.
    let (mut frame_stream, camera) = setup_stream_sized(&device_info, None).await;
    let camera = Arc::new(Mutex::new(camera));

    assert_eq!(
        frame_stream.params().packet_size,
        CAMERA_MAX,
        "the stream must follow the size the camera actually holds, not the one requested"
    );

    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let mut cam = cam.lock().unwrap();
        cam.acquisition_start().expect("start acquisition");
    })
    .await
    .unwrap();

    let frame = tokio::time::timeout(Duration::from_secs(5), frame_stream.next_frame())
        .await
        .expect("timeout waiting for frame — the clamped packet size was not followed")
        .expect("frame error")
        .expect("stream ended without a frame");

    // A stride disagreement of this kind does not merely drop frames; when one
    // does complete it is the wrong length, so assert the payload too.
    assert_eq!(
        frame.payload.len(),
        (frame.width * frame.height) as usize,
        "frame payload length must match width*height for Mono8"
    );

    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let mut cam = cam.lock().unwrap();
        cam.acquisition_stop().expect("stop acquisition");
    })
    .await
    .unwrap();
}

/// ADR-0021 / SR-14: default StreamBuilder must not rewrite GevSCPSPacketSize.
#[tokio::test]
async fn test_default_preserves_camera_packet_size() {
    const PRESET: u32 = 2500;

    let _cam = common::TestCamera::start().await;
    let device_info = discover_fake().await;

    let control_addr = std::net::SocketAddr::new(device_info.ip.into(), gige::GVCP_PORT);
    let mut device = gige::GigeDevice::open(control_addr)
        .await
        .expect("open device");
    device.claim_control().await.expect("claim control");
    device
        .set_stream_packet_size(0, PRESET)
        .await
        .expect("preset GevSCPSPacketSize");

    let stream = viva_genicam::StreamBuilder::new(&mut device)
        .iface(loopback_iface())
        .build()
        .await
        .expect("build with default preserve");

    assert_eq!(
        stream.params().packet_size,
        PRESET,
        "default must keep the camera register, not overwrite from NIC MTU"
    );
    let held = device
        .get_stream_packet_size(0)
        .await
        .expect("read back after preserve build");
    assert_eq!(held, PRESET, "preserve must not write the camera register");
}

/// Accumulates every `WARN` this test binary emits.
#[derive(Clone, Default)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogCapture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Install a **global** `WARN` capture, once per process.
///
/// A thread-local subscriber (`tracing::subscriber::set_default`) is not
/// enough. On Windows the receive loop runs on its own std thread inside
/// `windows_frame_receiver`, which never sees a subscriber installed on the
/// test's thread — the first Windows CI run of this test captured an empty log
/// for exactly that reason, while the wiring it checks was present and correct.
/// Assertions below are `contains`, so warnings interleaved from other tests
/// are harmless.
fn captured_warnings() -> &'static LogCapture {
    static CAPTURE: std::sync::OnceLock<LogCapture> = std::sync::OnceLock::new();
    CAPTURE.get_or_init(|| {
        let capture = LogCapture::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(capture.clone())
            .with_max_level(tracing::Level::WARN)
            // Colour escapes would sit between the words the assertions match.
            .with_ansi(false)
            .finish();
        tracing::subscriber::set_global_default(subscriber)
            .expect("no other test may install a global subscriber");
        capture
    })
}

/// The probe must find a ceiling that lives in the **path**, not the device —
/// backlog `SR-13`, reported as
/// [#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112).
///
/// The numbers are the reporter's, measured by hand on a Vieworks FS3200T: the
/// camera declares `Max=16366`, accepts and stores 16114 without complaint, and
/// streams nothing, because 9198 is the largest datagram the link carries
/// (`9198 + 18 = 9216`). Nothing readable from the device says so. `SR-02`'s
/// read-back returns 16114 and is perfectly correct; only a test packet finds
/// the real limit.
///
/// So the fake accepts any size into the register and drops anything larger
/// than 9198 in flight, and the probe has to land on exactly 9198.
#[tokio::test]
async fn test_probe_finds_a_path_ceiling_the_device_does_not_report() {
    const PATH_CEILING: u32 = 9198;
    // The reporter's host MTU. Requested explicitly rather than via `None`,
    // because `nic::mtu` probes only on Linux and Windows and returns a
    // hardcoded 1500 elsewhere (TC-11) — on macOS the auto path would ask for
    // 1500, which is below the probe floor, and the test would assert nothing.
    const HOST_MTU: u32 = 16114;

    let _cam = common::TestCamera::start_with(|b| b.max_on_wire(PATH_CEILING)).await;
    let device_info = discover_fake().await;

    let (mut frame_stream, camera) = setup_stream_sized(&device_info, Some(HOST_MTU)).await;
    let camera = Arc::new(Mutex::new(camera));

    assert_eq!(
        frame_stream.params().packet_size,
        PATH_CEILING,
        "the probe must bisect to the largest size the path actually carries"
    );

    // Landing on the right answer is only half of it: the device has to be left
    // *configured* at that answer. The fake now enforces its ceiling on the
    // stream as well as on test packets, so a camera left one byte above it
    // delivers nothing and this times out. Before the write-back, the bisection
    // on exactly these numbers negotiated 9198 and left the register at 9199 —
    // the first size the reporter measured as failing.
    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let mut cam = cam.lock().unwrap();
        cam.acquisition_start().expect("start acquisition");
    })
    .await
    .unwrap();

    let frame = tokio::time::timeout(Duration::from_secs(5), frame_stream.next_frame())
        .await
        .expect("timeout waiting for frame — the negotiated size never reached the camera")
        .expect("frame error")
        .expect("stream ended without a frame");

    assert_eq!(
        frame.payload.len(),
        (frame.width * frame.height) as usize,
        "frame payload length must match width*height for Mono8"
    );

    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let mut cam = cam.lock().unwrap();
        cam.acquisition_stop().expect("stop acquisition");
    })
    .await
    .unwrap();
}

/// Open a device, claim it, and build a stream, handing back the device so the
/// caller can read `GevSCPSPacketSize` afterwards.
///
/// [`setup_stream_sized`] moves the device into a `GigeRegisterIo`, and the only
/// way back to the register from there is a GenApi node read — which #112 showed
/// is a *different address* (`0x190A04` against the bootstrap `0x0D04`) and a
/// cached one. So the tests that care about the register open their own device.
async fn build_stream_keeping_device(
    device_info: &gige::DeviceInfo,
    configure: impl FnOnce(viva_genicam::StreamBuilder<'_>) -> viva_genicam::StreamBuilder<'_>,
) -> (viva_genicam::Stream, gige::GigeDevice) {
    use std::net::{IpAddr, SocketAddr};

    let control_addr = SocketAddr::new(IpAddr::V4(device_info.ip), gige::GVCP_PORT);
    let mut device = gige::GigeDevice::open(control_addr)
        .await
        .expect("open device");
    device.claim_control().await.expect("claim control");

    let builder = viva_genicam::StreamBuilder::new(&mut device).iface(loopback_iface());
    let stream = configure(builder).build().await.expect("build stream");

    (stream, device)
}

/// After probing, the device must hold the size the probe chose.
///
/// Asking for a test packet *is* a write to `GevSCPSPacketSize` — bit 31 rides
/// in the same register as the size — so a probe that does not put its answer
/// back leaves the camera at the last size it happened to *test*. The bisection
/// ends on a failing midpoint about half the time, and on the reporter's numbers
/// it ends on exactly 9199 while reporting 9198.
///
/// That is `SR-02` again from the other direction: host and camera disagree
/// about the stride, and this time we caused it. Asserting `params()` alone
/// cannot see it, which is why the original `SR-13` test passed throughout.
#[tokio::test]
async fn test_probe_leaves_the_device_holding_the_negotiated_size() {
    const PATH_CEILING: u32 = 9198;
    const HOST_MTU: u32 = 16114;

    let _cam = common::TestCamera::start_with(|b| b.max_on_wire(PATH_CEILING)).await;
    let device_info = discover_fake().await;

    let (stream, mut device) =
        build_stream_keeping_device(&device_info, |b| b.packet_size(HOST_MTU)).await;

    assert_eq!(stream.params().packet_size, PATH_CEILING);

    let held = device
        .get_stream_packet_size(0)
        .await
        .expect("read GevSCPSPacketSize back");
    assert_eq!(
        held, PATH_CEILING,
        "the device must be left holding the negotiated size, not the last one probed"
    );
}

/// A device that answers no test packet must be left as it was found.
///
/// The control probe writes [`PROBE_FLOOR`]-worth of bytes into
/// `GevSCPSPacketSize` before it can learn anything, so "change nothing" is not
/// the same as "return early" — the register has already moved. Without the
/// write-back, every camera that has never implemented test packets ends up
/// configured at 1500 while the host strides at the size it asked for, and no
/// frame completes. That is the common path, not an edge case.
#[tokio::test]
async fn test_probe_restores_the_requested_size_when_no_test_packet_returns() {
    const REQUESTED: u32 = 9000;

    let _cam = common::TestCamera::start_with(|b| b.max_on_wire(0)).await;
    let device_info = discover_fake().await;

    let (stream, mut device) =
        build_stream_keeping_device(&device_info, |b| b.packet_size(REQUESTED)).await;

    assert_eq!(stream.params().packet_size, REQUESTED);

    let held = device
        .get_stream_packet_size(0)
        .await
        .expect("read GevSCPSPacketSize back");
    assert_eq!(
        held, REQUESTED,
        "a silent device must keep the requested size, not the probe's control size"
    );
}

/// When no test packet comes back at all, the requested size stands.
///
/// This is the regression that matters most about `SR-13`. Most cameras have
/// worked for years without ever being asked for a test packet, and a probe
/// that read "no answer" as "too big" would walk every one of them down to
/// 1500 — turning a fix for one link into a throughput collapse everywhere
/// else. The control probe at the floor exists precisely so silence is
/// diagnosed as silence rather than as narrowness.
///
/// The fake is configured to drop every test packet, which is indistinguishable
/// on the wire from a device that ignores the request — and deliberately so:
/// the probe cannot tell those apart and must not need to. Both mean "no
/// evidence", and the safe reading of no evidence is to change nothing.
#[tokio::test]
async fn test_probe_leaves_the_size_alone_when_no_test_packet_returns() {
    let _cam = common::TestCamera::start_with(|b| b.max_on_wire(0)).await;
    let device_info = discover_fake().await;

    let (frame_stream, _camera) = setup_stream_sized(&device_info, Some(9000)).await;

    assert_eq!(
        frame_stream.params().packet_size,
        9000,
        "a device that answers no test packet must keep the requested size"
    );
}

/// Check the fake's GVSP **bytes** against the specification's field tables,
/// with none of our own receive path in between (backlog `TC-04`, ADR-0019).
///
/// `viva-gige`'s unit tests already assert the *parser* against golden bytes.
/// This is the other direction, and the one that catches the failure mode the
/// ADR exists for: producer and consumer agreeing with each other while both
/// disagree with the standard. That has happened three times here — the SCPS
/// overhead, unaligned READMEM, and the Discovery ACK MAC offset in #57 — and
/// each time every test passed.
///
/// So the datagrams are read from a plain `UdpSocket` this test binds itself,
/// configured through `GigeDevice` directly. `StreamBuilder`, `Stream`,
/// `FrameStream` and `gvsp::parse_packet` are all deliberately absent; a round
/// trip through them would prove only that they agree with the fake.
#[tokio::test]
async fn test_fake_gvsp_packets_match_spec_layout() {
    use tokio::net::UdpSocket;

    const PACKET_SIZE: u32 = 1500;
    // GevSCPSPacketSize counts the IP datagram, so a data packet carries
    // packet_size - (20 IP + 8 UDP + 8 GVSP) bytes of image.
    const PAYLOAD_STRIDE: usize = PACKET_SIZE as usize - 36;

    // A deliberately small frame: 8 192 Mono8 bytes is six data packets, five
    // full and one partial, which is every stride case the assertions below
    // check. The default 640x480 is 210 packets, and a block is only usable
    // here if *all* of them arrive — on Windows CI loopback dropped at least
    // one from every block for 20 s and the test timed out with no block
    // captured. Fewer packets per block is what makes a whole block likely,
    // not a longer deadline.
    const WIDTH: u32 = 256;
    const HEIGHT: u32 = 32;

    let _cam = common::TestCamera::start_with(|builder| builder.width(WIDTH).height(HEIGHT)).await;
    let device_info = discover_fake().await;
    let control_addr = std::net::SocketAddr::new(device_info.ip.into(), gige::GVCP_PORT);

    let xml = viva_genapi_xml::fetch_and_load_xml({
        let addr = control_addr;
        move |address, length| async move {
            let mut dev = gige::GigeDevice::open(addr)
                .await
                .map_err(|e| viva_genapi_xml::XmlError::Transport(e.to_string()))?;
            dev.read_mem(address, length)
                .await
                .map_err(|e| viva_genapi_xml::XmlError::Transport(e.to_string()))
        }
    })
    .await
    .expect("fetch XML");
    let model = viva_genapi_xml::parse(&xml).expect("parse XML");
    let nodemap = viva_genicam::genapi::NodeMap::try_from_xml(model).expect("build nodemap");

    // Our own socket, so nothing of ours touches the bytes.
    let sink = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind GVSP sink");
    let sink_port = sink.local_addr().expect("sink addr").port();

    let mut device = gige::GigeDevice::open(control_addr)
        .await
        .expect("open device");
    device.claim_control().await.expect("claim control");
    device
        .set_stream_destination(0, std::net::Ipv4Addr::LOCALHOST, sink_port)
        .await
        .expect("set stream destination");
    device
        .set_stream_packet_size(0, PACKET_SIZE)
        .await
        .expect("set packet size");

    let handle = tokio::runtime::Handle::current();
    let camera = Arc::new(Mutex::new(Camera::new(
        GigeRegisterIo::new(handle, device),
        nodemap,
    )));

    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        cam.lock().unwrap().acquisition_start().expect("start");
    })
    .await
    .unwrap();

    // Collect blocks until one arrives whole.
    //
    // UDP on loopback does drop datagrams: Windows CI lost a trailer here on
    // the first run, and the collector ran on into the next block and compared
    // its packets against the previous block's id. An incomplete block is a
    // flaky test rather than a real finding, so discard it and follow the next
    // one — while keeping every assertion strict on the block that is kept.
    //
    // The completeness filter is deliberately weak: it checks only that the
    // trailer's packet id accounts for every data packet, i.e. that nothing
    // went missing. Each packet's own id, the stride and the byte total are
    // asserted below and are not implied by that count.
    let block_id_of = |pkt: &[u8]| u16::from_be_bytes([pkt[2], pkt[3]]);
    let packet_id_of = |pkt: &[u8]| u32::from_be_bytes([0, pkt[5], pkt[6], pkt[7]]);

    /// One GVSP block as it arrived: leader, data packets in receipt order,
    /// trailer.
    struct Block {
        leader: Vec<u8>,
        payloads: Vec<Vec<u8>>,
        trailer: Vec<u8>,
    }

    let mut captured: Option<Block> = None;
    let mut leader: Vec<u8> = Vec::new();
    let mut payloads: Vec<Vec<u8>> = Vec::new();
    let mut buf = vec![0u8; 4096];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while captured.is_none() && tokio::time::Instant::now() < deadline {
        let Ok(Ok((len, _))) =
            tokio::time::timeout(Duration::from_secs(3), sink.recv_from(&mut buf)).await
        else {
            break;
        };
        let pkt = buf[..len].to_vec();
        // Byte 4: the low nibble is the packet format, the high bit is the
        // extended-block-ID flag. Dispatch on the nibble so a future fake that
        // sets the flag fails the explicit assertion below rather than timing
        // out here with nothing to say.
        let same_block = !leader.is_empty() && block_id_of(&pkt) == block_id_of(&leader);
        match pkt[4] & 0x0F {
            // A leader always begins a fresh block; if one was in progress it
            // lost its trailer.
            0x01 => {
                leader = pkt;
                payloads.clear();
            }
            0x03 if same_block => payloads.push(pkt),
            0x02 if same_block => {
                if packet_id_of(&pkt) == payloads.len() as u32 + 1 {
                    captured = Some(Block {
                        leader: std::mem::take(&mut leader),
                        payloads: std::mem::take(&mut payloads),
                        trailer: pkt,
                    });
                } else {
                    leader.clear();
                    payloads.clear();
                }
            }
            _ => {}
        }
    }
    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let _ = cam.lock().unwrap().acquisition_stop();
    })
    .await
    .unwrap();

    let Block {
        leader,
        payloads,
        trailer,
    } = captured.expect("no complete GVSP block arrived within the deadline");
    assert!(!payloads.is_empty(), "a block with no data packets");

    let be16 = |b: &[u8], at: usize| u16::from_be_bytes([b[at], b[at + 1]]);
    let be32 = |b: &[u8], at: usize| u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);

    // ── Data Leader ────────────────────────────────────────────────────────
    // Standard 8-byte header: status(2) block_id(2) format(1) packet_id(3),
    // then a 36-byte image leader payload.
    assert_eq!(leader.len(), 44, "8-byte header + 36-byte image leader");
    assert_eq!(be16(&leader, 0), 0x0000, "status at offset 0");
    let block_id = be16(&leader, 2);
    assert_ne!(block_id, 0, "block id at offset 2 (0 is reserved)");
    assert_eq!(leader[4], 0x01, "packet format at offset 4: leader");
    assert_eq!(
        u32::from_be_bytes([0, leader[5], leader[6], leader[7]]),
        0,
        "the leader is packet 0 of its block"
    );
    assert_eq!(be16(&leader, 8), 0, "reserved at offset 8");
    assert_eq!(
        be16(&leader, 10),
        0x0001,
        "payload type at offset 10: image"
    );
    // Offset 12..20 is the timestamp; the fake's clock is free-running, so
    // assert only that it is set rather than pinning a value.
    assert_ne!(
        u64::from_be_bytes(leader[12..20].try_into().unwrap()),
        0,
        "timestamp at offset 12"
    );
    assert_eq!(
        be32(&leader, 20),
        0x0108_0001,
        "pixel format at offset 20: Mono8"
    );
    assert_eq!(be32(&leader, 24), WIDTH, "size_x at offset 24");
    assert_eq!(be32(&leader, 28), HEIGHT, "size_y at offset 28");
    assert_eq!(be32(&leader, 32), 0, "offset_x at offset 32");
    assert_eq!(be32(&leader, 36), 0, "offset_y at offset 36");
    assert_eq!(be16(&leader, 40), 0, "padding_x at offset 40");
    assert_eq!(be16(&leader, 42), 0, "padding_y at offset 42");

    // ── Data Payload ───────────────────────────────────────────────────────
    // Image bytes begin immediately after the 8-byte header — no payload-type
    // field, unlike the leader and trailer.
    for (i, pkt) in payloads.iter().enumerate() {
        assert_eq!(be16(pkt, 0), 0x0000, "status, packet {i}");
        assert_eq!(be16(pkt, 2), block_id, "same block id, packet {i}");
        assert_eq!(pkt[4], 0x03, "packet format: payload, packet {i}");
        assert_eq!(
            u32::from_be_bytes([0, pkt[5], pkt[6], pkt[7]]),
            i as u32 + 1,
            "data packets are numbered from 1, the leader being 0"
        );
    }
    // Every packet but the last carries a full stride. This is the accounting
    // that was wrong in both the fake and the receiver at once — they
    // subtracted different overheads and agreed anyway, because each only ever
    // talked to the other.
    for pkt in &payloads[..payloads.len() - 1] {
        assert_eq!(
            pkt.len() - 8,
            PAYLOAD_STRIDE,
            "a full data packet carries packet_size - 36 image bytes"
        );
    }
    let total: usize = payloads.iter().map(|p| p.len() - 8).sum();
    assert_eq!(
        total,
        (WIDTH * HEIGHT) as usize,
        "the data packets must carry exactly one Mono8 frame"
    );

    // ── Data Trailer ───────────────────────────────────────────────────────
    assert_eq!(be16(&trailer, 0), 0x0000, "status at offset 0");
    assert_eq!(be16(&trailer, 2), block_id, "same block id");
    assert_eq!(trailer[4], 0x02, "packet format at offset 4: trailer");
    assert_eq!(
        u32::from_be_bytes([0, trailer[5], trailer[6], trailer[7]]),
        payloads.len() as u32 + 1,
        "the trailer closes the block after the last data packet"
    );
    assert_eq!(be16(&trailer, 8), 0, "reserved at offset 8");
    assert_eq!(
        be16(&trailer, 10),
        0x0001,
        "payload type at offset 10, not offset 2 — the TC-17 defect"
    );
    assert_eq!(be32(&trailer, 12), HEIGHT, "size_y at offset 12");
}

/// The DX-09 warning must actually reach the log.
///
/// `SilenceWatch` itself is unit-tested; what this covers is the wiring into
/// the receive loop — which differs by platform, and needed a poll deadline on
/// `recv` off Windows before the loop could turn at all while nothing arrived.
///
/// The stream is built and then never given an `AcquisitionStart`, so not one
/// GVSP datagram arrives — the shape of a firewall block or a control privilege
/// held elsewhere, which is what the "no GVSP packet" verdict names.
#[tokio::test]
async fn test_silent_stream_warns_and_names_the_candidates() {
    let capture = captured_warnings();

    let _cam = common::TestCamera::start().await;
    let device_info = discover_fake().await;

    let (mut frame_stream, _camera) = setup_stream_sized(&device_info, Some(1500)).await;

    // Acquisition is deliberately never started, so this can only time out.
    let outcome = tokio::time::timeout(Duration::from_secs(5), frame_stream.next_frame()).await;
    assert!(
        outcome.is_err(),
        "a camera that was never told to acquire must not produce a frame"
    );

    let log = String::from_utf8_lossy(&capture.0.lock().unwrap().clone()).into_owned();
    assert!(
        log.contains("no GVSP packet has arrived"),
        "the receive loop must emit the DX-09 warning; captured log was:\n{log}"
    );
    // The value of the warning is the list, not the fact of it.
    for candidate in ["firewall", "control privilege", "trigger", "1500"] {
        assert!(
            log.contains(candidate),
            "the warning must name '{candidate}'; captured log was:\n{log}"
        );
    }
}

/// An explicitly configured packet size must be bounded like the probed one.
///
/// `best_packet_size` clamps the MTU to what an IPv4 datagram can carry, but
/// `--packet-size` bypassed that (the leftover in backlog TC-08) and reached
/// `GevSCPSPacketSize`, whose size field is 16 bits — so `--packet-size 70000`
/// silently configured 4 464 and produced a stream nobody could explain.
#[tokio::test]
async fn test_oversized_packet_size_is_refused_not_truncated() {
    let _cam = common::TestCamera::start().await;
    let device_info = discover_fake().await;

    let control_addr = std::net::SocketAddr::new(device_info.ip.into(), gige::GVCP_PORT);
    let mut device = gige::GigeDevice::open(control_addr)
        .await
        .expect("open device");
    device.claim_control().await.expect("claim control");

    // `Stream` is not `Debug`, so unwrap the result by hand rather than via
    // `expect_err`.
    let message = match viva_genicam::StreamBuilder::new(&mut device)
        .iface(loopback_iface())
        .packet_size(70_000)
        .build()
        .await
    {
        Ok(_) => panic!("a packet size wider than GevSCPSPacketSize must be refused"),
        Err(err) => err.to_string(),
    };
    assert!(
        message.contains("70000") && message.contains("65535"),
        "the error must name both the value and the bound, got: {message}"
    );
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

    // Mono8: the payload must be exactly width*height bytes. A mismatch means
    // the reassembly stride disagrees with the sender's packet chunking
    // (e.g. GevSCPSPacketSize header-overhead accounting).
    let expected_len = (frame.width * frame.height) as usize;
    assert_eq!(
        frame.payload.len(),
        expected_len,
        "frame payload length must match width*height for Mono8"
    );
    // The test pattern is non-constant; an all-identical payload means the
    // image data never actually landed in the buffer.
    let first = frame.payload[0];
    assert!(
        frame.payload.iter().any(|&b| b != first),
        "frame payload should contain a non-constant test pattern"
    );

    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let mut cam = cam.lock().unwrap();
        cam.acquisition_stop().expect("stop acquisition");
    })
    .await
    .unwrap();
}

/// Chunk data survives the whole round trip: enable it through GenApi, receive
/// a frame, decode the trailer.
///
/// Nothing exercised this path before. `Camera::configure_chunks` calls
/// `set_bool` on `ChunkModeActive` — correct, SFNC defines it as IBoolean — but
/// `viva-fake-gige` declared it as `<Integer>`, so the call failed with a type
/// mismatch and the only camera the project can test against could not turn
/// chunks on. That is why TC-17, which made chunks undecodable on *every*
/// conforming camera, was found in a user's log rather than here (backlog
/// TC-19).
///
/// What this proves, precisely: `ChunkModeActive`/`ChunkEnable` are writable as
/// booleans, the trailer's chunk region is located at the offset TC-17 fixed,
/// and two chunk entries decode into typed values. What it does **not** prove
/// is the byte order of a chunk *value* — the fake writes them little-endian
/// and `chunks.rs` reads them little-endian, so the two agree with each other
/// and a real camera is not consulted. That is TC-06, and it needs the spec or
/// hardware, not this test.
#[tokio::test]
async fn test_chunk_data_round_trips_through_genapi() {
    use viva_genicam::{ChunkConfig, ChunkKind, ChunkValue};

    let _cam = common::TestCamera::start().await;
    let device_info = discover_fake().await;

    let (mut frame_stream, camera) = setup_stream(&device_info).await;

    // Turn chunk mode on through the nodemap, exactly as an application would.
    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let mut cam = cam.lock().unwrap();
        cam.configure_chunks(&ChunkConfig {
            selectors: vec!["Timestamp".to_string(), "ExposureTime".to_string()],
            active: true,
        })
        .expect("configure chunks");
        cam.acquisition_start().expect("start acquisition");
    })
    .await
    .unwrap();

    let frame = tokio::time::timeout(Duration::from_secs(5), frame_stream.next_frame())
        .await
        .expect("timeout waiting for frame")
        .expect("frame error")
        .expect("stream ended without a frame");

    // The image itself must be unaffected by the chunk region that follows it.
    assert_eq!(
        frame.payload.len(),
        (frame.width * frame.height) as usize,
        "chunk mode must not change the image payload length for Mono8"
    );

    let chunks = frame
        .chunks
        .as_ref()
        .expect("frame carried no chunk map with chunk mode active");

    match frame.chunk(ChunkKind::Timestamp) {
        Some(ChunkValue::U64(ts)) => assert!(*ts > 0, "timestamp chunk should be non-zero"),
        other => panic!("expected a U64 timestamp chunk, got {other:?}"),
    }
    match frame.chunk(ChunkKind::ExposureTime) {
        Some(ChunkValue::F64(us)) => {
            assert!(
                us.is_finite() && *us > 0.0,
                "exposure chunk should be a positive finite value, got {us}"
            );
        }
        other => panic!("expected an F64 exposure chunk, got {other:?}"),
    }

    // A desynchronised chunk region decodes as a run of `Unknown` ids rather
    // than failing outright, which is how TC-17 stayed invisible for so long.
    let unknown: Vec<_> = chunks
        .keys()
        .filter(|k| matches!(k, ChunkKind::Unknown(_)))
        .collect();
    assert!(
        unknown.is_empty(),
        "chunk region parsed at the wrong offset: unexpected ids {unknown:?}"
    );

    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let mut cam = cam.lock().unwrap();
        cam.acquisition_stop().expect("stop acquisition");
    })
    .await
    .unwrap();
}

/// With chunk mode off, the trailer carries no chunk region at all.
///
/// The negative control for the test above: it is what makes a passing
/// `test_chunk_data_round_trips_through_genapi` mean chunks were *enabled*
/// rather than always present.
#[tokio::test]
async fn test_no_chunks_when_chunk_mode_is_off() {
    let _cam = common::TestCamera::start().await;
    let device_info = discover_fake().await;

    let (mut frame_stream, camera) = setup_stream(&device_info).await;

    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let mut cam = cam.lock().unwrap();
        cam.acquisition_start().expect("start acquisition");
    })
    .await
    .unwrap();

    let frame = tokio::time::timeout(Duration::from_secs(5), frame_stream.next_frame())
        .await
        .expect("timeout waiting for frame")
        .expect("frame error")
        .expect("stream ended without a frame");

    assert!(
        frame.chunks.as_ref().is_none_or(|c| c.is_empty()),
        "chunk mode is off, so the trailer must carry no chunk entries"
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
    assert_eq!(
        frame.payload.len(),
        (expected_w * expected_h) as usize,
        "Mono8 payload length must match Width*Height"
    );

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
        assert_eq!(
            frame.payload.len(),
            (frame.width * frame.height) as usize,
            "Mono8 payload length must match width*height on frame {i}"
        );
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

// NOTE: FORCEIP integration test removed -- UDP broadcast from a loopback-bound
// socket does not reliably reach the fake camera across platforms (fails on macOS
// outright, times out on some Linux CI runners). The FORCEIP payload encoding is
// validated by the unit test `forceip_payload_encoding` in viva-gige/src/gvcp.rs.

// ── Action commands and events (TC-02, TC-03, TC-07) ────────────────────────

/// A matching `ACTION_CMD` is acknowledged; a non-matching one is ignored.
///
/// This is the test that could not exist before: the fake had no action
/// handler at all, so the client's opcode (0x0080 — `READREG`) and its 24-byte
/// payload were never contradicted by anything.
#[tokio::test]
async fn test_action_command_is_acknowledged() {
    let _cam = common::TestCamera::start().await;

    let params = gige::action::ActionParams {
        device_key: viva_fake_gige::FAKE_DEVICE_KEY,
        group_key: viva_fake_gige::FAKE_GROUP_KEY,
        group_mask: viva_fake_gige::FAKE_GROUP_MASK,
        scheduled_time: None,
    };
    let dest = std::net::SocketAddr::from(([127, 0, 0, 1], gige::GVCP_PORT));
    let summary = gige::action::send_action(dest, &params, 1000)
        .await
        .expect("send action");
    assert_eq!(summary.sent, 1);
    assert_eq!(
        summary.acks, 1,
        "fake camera did not acknowledge the action"
    );

    // A device the command is not addressed to stays silent.
    let other = gige::action::ActionParams {
        group_key: viva_fake_gige::FAKE_GROUP_KEY ^ 0xFFFF,
        ..params
    };
    let summary = gige::action::send_action(dest, &other, 300)
        .await
        .expect("send action");
    assert_eq!(
        summary.acks, 0,
        "camera answered an action addressed to another group"
    );
}

/// The fake emits `GEV_EVENT_START_OF_TRANSFER` per frame; the client decodes
/// it off the wire with the right event id and a non-zero device timestamp.
///
/// Exercises the whole path: `EventSelector`/`EventNotification` through
/// GenApi, the message-channel bootstrap registers, `EVENT_CMD` on the wire,
/// and `EventStream`. None of it had a test before, because the fake emitted
/// no events at all.
#[tokio::test(flavor = "multi_thread")]
async fn test_event_stream_receives_start_of_transfer() {
    let _cam = common::TestCamera::start().await;
    let device_info = discover_fake().await;

    let (_frame_stream, mut camera) = setup_stream_owned(&device_info).await;
    let local: std::net::Ipv4Addr = [127, 0, 0, 1].into();
    let port = 10_020u16;

    // Enable two events, and expect the first one to still be enabled.
    // `EventNotification` is selected by `EventSelector`, so this writes the
    // same register twice; a device backing it with one word would answer
    // only for `EndOfTransfer` and this test would time out.
    camera
        .configure_events(local, port, &["StartOfTransfer", "EndOfTransfer"])
        .await
        .expect("configure events");
    let events = camera
        .open_event_stream(local, port)
        .await
        .expect("open event stream");

    let camera = Arc::new(Mutex::new(camera));
    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        cam.lock().unwrap().acquisition_start().expect("start");
    })
    .await
    .unwrap();

    let event = tokio::time::timeout(Duration::from_secs(5), events.next())
        .await
        .expect("timed out waiting for an event")
        .expect("event");

    // 0x0005 is GEV_EVENT_START_OF_TRANSFER. A parser reading the id out of
    // the reserved word — as ours did — sees 0 here.
    assert_eq!(event.id, 0x0005, "event identifier");
    assert!(event.ts_dev > 0, "device timestamp should not be zero");
    assert!(event.data.is_empty(), "EVENT_CMD carries no event data");

    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        cam.lock().unwrap().acquisition_stop().expect("stop");
    })
    .await
    .unwrap();
}

/// A camera with no `EventSelector`/`EventNotification` gets a clear error
/// rather than a write to an invented "notification mask" register.
#[tokio::test(flavor = "multi_thread")]
async fn test_enabling_unknown_event_fails_loudly() {
    let _cam = common::TestCamera::start().await;
    let device = discover_fake().await;

    let (mut camera, _xml) = connect_gige_with_xml(&device).await.expect("connect");
    let err = camera
        .configure_events([127, 0, 0, 1].into(), 10_021, &["NoSuchEvent"])
        .await
        .expect_err("enabling an unknown event should fail");
    let text = err.to_string();
    assert!(
        text.contains("NoSuchEvent"),
        "error should name the event: {text}"
    );
}

/// Issue #140 / #112: an 8-byte register declared `<Sign>Unsigned</Sign>` whose
/// top bit is set. GenApi integers are `int64`, so there is nowhere else for
/// the value to go — it is reinterpreted as two's complement rather than
/// refused, which is what makes the node readable at all.
///
/// The fake's node is copied in shape from the FLIR BFS-PGE-31S4C-C
/// description in the vendor corpus, which is the model #140 was reported on.
#[tokio::test]
async fn test_full_width_unsigned_register_is_readable() {
    let _cam = common::TestCamera::start().await;
    let camera = connect_fake().await;

    // Asserted against the bytes the fake stores, not against our own encoder:
    // a round trip through the codec under test only proves it agrees with
    // itself.
    let raw = blocking_read_register(&camera, 0x20078, 8).await;
    assert_eq!(
        raw,
        vec![0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xED, 0x29, 0x79],
        "fake register bytes are not the expected two's-complement pattern"
    );

    let value = blocking_get(&camera, "GevIEEE1588OffsetFromMasterLatched_Val")
        .await
        .expect("read latched PTP offset");
    assert_eq!(value, "-1234567");
}

/// GA-28: a plain `<IntReg>` declaring `<Endianess>LittleEndian</Endianess>`.
/// 311 such declarations across 16 of the 38 corpus documents were decoded
/// big-endian, because `NodeDecl::Integer` had nowhere to record the order and
/// the bitfield that would have carried it does not exist on an unmasked
/// register.
#[tokio::test]
async fn test_little_endian_registers_are_not_byte_swapped() {
    let _cam = common::TestCamera::start().await;
    let camera = connect_fake().await;

    // 0x12345678 little-endian on the wire. Read big-endian it is 0x78563412,
    // which is a plausible number and not an error — which is why this went
    // unnoticed.
    let raw = blocking_read_register(&camera, 0x20118, 4).await;
    assert_eq!(raw, vec![0xA0, 0x05, 0x00, 0x00]);
    let width = blocking_get(&camera, "ChunkWidth_Val")
        .await
        .expect("read chunk width");
    assert_eq!(
        width, "1440",
        "little-endian 4-byte register was byte-swapped"
    );

    let raw = blocking_read_register(&camera, 0x20110, 8).await;
    assert_eq!(raw, vec![0x78, 0x56, 0x34, 0x12, 0x00, 0x00, 0x00, 0x00]);
    let ticks = blocking_get(&camera, "ChunkTimestamp_Val")
        .await
        .expect("read chunk timestamp");
    assert_eq!(
        ticks, "305419896",
        "little-endian 8-byte register was byte-swapped"
    );
}
