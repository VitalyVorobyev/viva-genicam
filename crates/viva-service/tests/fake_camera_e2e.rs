//! End-to-end integration test: fake camera -> viva-service -> Zenoh client.
//!
//! Uses the in-process `viva-fake-gige` camera -- no external tools required.
//!
//! ```sh
//! cargo test -p viva-service --test fake_camera_e2e
//! ```

use std::net::Ipv4Addr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use viva_service::acquisition;
use viva_service::device::{DeviceHandle, DeviceOps};
use viva_service::nodes;
use viva_service::status;
use viva_service::xml;

use tokio::sync::{Mutex, OwnedMutexGuard, watch};
use viva_fake_gige::{FakeCamera, FakeCameraBuilder, GvcpCommand};
use viva_zenoh_api::frame_header::FrameHeader;
use viva_zenoh_api::{
    AcquisitionCommand, AcquisitionControlRequest, EvsFormat, EvsHeader, NodeOpResponse, keys,
};

// ---------------------------------------------------------------------------
// Fake camera guard with global port lock
// ---------------------------------------------------------------------------

static CAMERA_LOCK: OnceLock<Arc<Mutex<()>>> = OnceLock::new();

fn camera_lock() -> Arc<Mutex<()>> {
    CAMERA_LOCK.get_or_init(|| Arc::new(Mutex::new(()))).clone()
}

struct TestCamera {
    camera: Option<FakeCamera>,
    _guard: OwnedMutexGuard<()>,
}

impl TestCamera {
    async fn start() -> Self {
        Self::start_with(|builder| builder).await
    }

    /// Start a fake camera with extra builder customization on top of the
    /// standard loopback configuration.
    async fn start_with<F>(customize: F) -> Self
    where
        F: Fn(FakeCameraBuilder) -> FakeCameraBuilder,
    {
        let guard = camera_lock().lock_owned().await;
        let camera = loop {
            let builder = FakeCamera::builder()
                .bind_ip([127, 0, 0, 1].into())
                .port(3956)
                .width(640)
                .height(480)
                .fps(30);
            match customize(builder).build().await {
                Ok(cam) => break cam,
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
                Err(e) => panic!("failed to start fake camera: {e}"),
            }
        };
        TestCamera {
            camera: Some(camera),
            _guard: guard,
        }
    }
}

impl Drop for TestCamera {
    fn drop(&mut self) {
        // `FakeCamera::stop` is async; from a sync Drop we rely on
        // `FakeCamera`'s own Drop impl to abort the tasks best-effort.
        self.camera.take();
    }
}

/// Resolve the loopback interface (platform-independent).
fn loopback_iface() -> viva_genicam::gige::nic::Iface {
    viva_genicam::gige::nic::Iface::from_ipv4(std::net::Ipv4Addr::LOCALHOST)
        .expect("loopback iface")
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Spawn the same set of per-device tasks that viva-service normally runs.
async fn spawn_service_tasks(
    session: Arc<zenoh::Session>,
    device: Arc<DeviceHandle>,
    shutdown: watch::Receiver<bool>,
) -> Vec<tokio::task::JoinHandle<()>> {
    let device_id = device.device_id().to_string();

    status::publish_connected(&session, &device_id).await;
    nodes::publish_initial_values(&session, device.as_ref()).await;

    vec![
        tokio::spawn(xml::run(
            session.clone(),
            device_id.clone(),
            device.raw_xml().to_string(),
            shutdown.clone(),
        )),
        tokio::spawn(nodes::run_set_queryable(
            session.clone(),
            device.clone(),
            shutdown.clone(),
        )),
        tokio::spawn(nodes::run_execute_queryable(
            session.clone(),
            device.clone(),
            shutdown.clone(),
        )),
        tokio::spawn(nodes::run_bulk_read_queryable(
            session.clone(),
            device.clone(),
            shutdown.clone(),
        )),
        tokio::spawn(acquisition::run(
            session.clone(),
            device.clone(),
            shutdown.clone(),
        )),
    ]
}

/// Send an acquisition control request and return the response.
async fn send_acq_command(
    session: &zenoh::Session,
    device_id: &str,
    command: AcquisitionCommand,
) -> NodeOpResponse {
    let key = keys::acquisition_control(device_id);
    let req = AcquisitionControlRequest { command };
    let payload = serde_json::to_vec(&req).unwrap();

    let replies = session
        .get(&key)
        .payload(payload)
        .timeout(Duration::from_secs(10))
        .await
        .expect("GET failed");

    let reply = replies.recv_async().await.expect("no reply received");
    let sample = reply.result().expect("query error");
    serde_json::from_slice::<NodeOpResponse>(&sample.payload().to_bytes())
        .expect("failed to parse NodeOpResponse")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Full round-trip: discover -> connect -> stream -> receive frames -> stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_acquisition_roundtrip() {
    let _cam = TestCamera::start().await;

    // 1. Open a Zenoh session.
    let session = Arc::new(zenoh::open(zenoh::Config::default()).await.unwrap());

    // 2. Discover the fake camera on loopback.
    let devices = viva_genicam::gige::discover_all(Duration::from_secs(2))
        .await
        .expect("discovery failed");
    let dev_info = devices
        .iter()
        .find(|d| d.ip == Ipv4Addr::LOCALHOST)
        .expect("fake camera not found on loopback");

    // 3. Connect and create DeviceHandle.
    let iface = loopback_iface();
    let handle = Arc::new(
        DeviceHandle::connect(dev_info, Some(iface))
            .await
            .expect("connect failed"),
    );
    let device_id = handle.device_id().to_string();

    // 4. Spawn service tasks (replicates main.rs logic).
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let tasks = spawn_service_tasks(session.clone(), handle.clone(), shutdown_rx).await;

    // Give queryables a moment to register.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // 5. Subscribe to the image topic.
    let image_key = keys::image(&device_id);
    let subscriber = session.declare_subscriber(&image_key).await.unwrap();

    // 6. Start acquisition.
    let resp = send_acq_command(&session, &device_id, AcquisitionCommand::Start).await;
    assert!(resp.ok, "AcquisitionStart failed: {:?}", resp.error);

    // 7. Receive at least one frame.
    let sample = tokio::time::timeout(Duration::from_secs(5), subscriber.recv_async())
        .await
        .expect("timeout waiting for frame on Zenoh")
        .expect("subscriber closed");

    let payload = sample.payload().to_bytes();
    assert!(
        payload.len() > 16,
        "frame payload too small: {} bytes",
        payload.len()
    );

    // Decode frame header.
    let (header, pixel_data) = FrameHeader::decode(&payload).expect("frame header decode failed");
    assert!(header.width > 0, "frame width should be > 0");
    assert!(header.height > 0, "frame height should be > 0");
    assert!(!pixel_data.is_empty(), "pixel data should not be empty");
    eprintln!(
        "E2E frame: {}x{} seq={} pixel_data={}B",
        header.width,
        header.height,
        header.seq,
        pixel_data.len()
    );

    // 8. Stop acquisition.
    let resp = send_acq_command(&session, &device_id, AcquisitionCommand::Stop).await;
    assert!(resp.ok, "AcquisitionStop failed: {:?}", resp.error);

    // 9. Verify streaming has stopped (no frames within 2 s).
    let late = tokio::time::timeout(Duration::from_secs(2), subscriber.recv_async()).await;
    if late.is_ok() {
        let drain_result =
            tokio::time::timeout(Duration::from_secs(2), subscriber.recv_async()).await;
        if drain_result.is_ok() {
            let drain_result2 =
                tokio::time::timeout(Duration::from_secs(2), subscriber.recv_async()).await;
            assert!(
                drain_result2.is_err(),
                "frames should stop after AcquisitionStop"
            );
        }
    }

    // 10. Clean shutdown.
    let _ = shutdown_tx.send(true);
    for task in tasks {
        let _ = tokio::time::timeout(Duration::from_secs(3), task).await;
    }
}

/// Verify that starting acquisition twice returns an error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_double_start_rejected() {
    let _cam = TestCamera::start().await;

    let session = Arc::new(zenoh::open(zenoh::Config::default()).await.unwrap());
    let devices = viva_genicam::gige::discover_all(Duration::from_secs(2))
        .await
        .unwrap();
    let dev_info = devices
        .iter()
        .find(|d| d.ip == Ipv4Addr::LOCALHOST)
        .unwrap();

    let handle = Arc::new(
        DeviceHandle::connect(dev_info, Some(loopback_iface()))
            .await
            .unwrap(),
    );
    let device_id = handle.device_id().to_string();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let tasks = spawn_service_tasks(session.clone(), handle.clone(), shutdown_rx).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // First start should succeed.
    let resp = send_acq_command(&session, &device_id, AcquisitionCommand::Start).await;
    assert!(resp.ok);

    // Second start should fail.
    let resp = send_acq_command(&session, &device_id, AcquisitionCommand::Start).await;
    assert!(!resp.ok, "double start should be rejected");

    // Stop to clean up.
    let _ = send_acq_command(&session, &device_id, AcquisitionCommand::Stop).await;
    let _ = shutdown_tx.send(true);
    for task in tasks {
        let _ = tokio::time::timeout(Duration::from_secs(3), task).await;
    }
}

/// Verify a running acquisition remains live beyond the heartbeat window.
///
/// The service no longer runs a keepalive of its own — the transport owns it —
/// so this now exercises the library's, through the full Zenoh path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_sustained_streaming() {
    let _cam = TestCamera::start().await;

    let session = Arc::new(zenoh::open(zenoh::Config::default()).await.unwrap());
    let devices = viva_genicam::gige::discover_all(Duration::from_secs(2))
        .await
        .expect("discovery failed");
    let dev_info = devices
        .iter()
        .find(|d| d.ip == Ipv4Addr::LOCALHOST)
        .expect("fake camera not found on loopback");

    let handle = Arc::new(
        DeviceHandle::connect(dev_info, Some(loopback_iface()))
            .await
            .expect("connect failed"),
    );
    let device_id = handle.device_id().to_string();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let tasks = spawn_service_tasks(session.clone(), handle.clone(), shutdown_rx).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let image_key = keys::image(&device_id);
    let subscriber = session.declare_subscriber(&image_key).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = send_acq_command(&session, &device_id, AcquisitionCommand::Start).await;
    assert!(resp.ok, "AcquisitionStart failed: {:?}", resp.error);

    let stream_duration = Duration::from_secs(6);
    let max_allowed_gap = Duration::from_secs(3);
    let deadline = tokio::time::Instant::now() + stream_duration;
    let mut frame_count: u64 = 0;
    let mut last_frame = tokio::time::Instant::now();
    let mut max_observed_gap = Duration::ZERO;

    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(5), subscriber.recv_async()).await {
            Ok(Ok(sample)) => {
                let payload = sample.payload().to_bytes();
                assert!(
                    payload.len() > 16,
                    "frame payload too small during sustained stream: {} bytes",
                    payload.len()
                );
                let now = tokio::time::Instant::now();
                let gap = now - last_frame;
                if gap > max_observed_gap {
                    max_observed_gap = gap;
                }
                last_frame = now;
                frame_count += 1;
            }
            Ok(Err(e)) => panic!("subscriber closed unexpectedly: {e}"),
            Err(_) => panic!(
                "no frame received for 5 s during sustained stream; frames so far: {frame_count}"
            ),
        }
    }

    assert!(
        frame_count > 50,
        "expected >50 frames in 6 s, got {frame_count}"
    );
    assert!(
        max_observed_gap < max_allowed_gap,
        "max inter-frame gap {:?} exceeds {:?}; total frames: {frame_count}",
        max_observed_gap,
        max_allowed_gap,
    );

    let resp = send_acq_command(&session, &device_id, AcquisitionCommand::Stop).await;
    assert!(resp.ok, "AcquisitionStop failed: {:?}", resp.error);

    let _ = shutdown_tx.send(true);
    for task in tasks {
        let _ = tokio::time::timeout(Duration::from_secs(3), task).await;
    }
}

/// Verify that `DeviceHandle::get_feature_state` drives `is_implemented`,
/// `is_available`, `access_mode`, and `enum_available` off live predicates
/// rather than hardcoded defaults. This is the service-layer counterpart of
/// `crates/viva-genicam/tests/predicates.rs` — what flips here is the wire
/// contract the UI consumes, driven by the same realistic predicates:
/// `ExposureAuto` locks `ExposureTime`, `AcquisitionFrameRateEnable` gates
/// `AcquisitionFrameRate` availability, and `SensorType` filters
/// `PixelFormat` entries.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_feature_state_reflects_predicates() {
    let _cam = TestCamera::start().await;

    let devices = viva_genicam::gige::discover_all(Duration::from_secs(2))
        .await
        .unwrap();
    let dev_info = devices
        .iter()
        .find(|d| d.ip == Ipv4Addr::LOCALHOST)
        .expect("fake camera not found on loopback");

    let handle = DeviceHandle::connect(dev_info, Some(loopback_iface()))
        .await
        .expect("connect failed");

    // --- ExposureTime unlocked ---------------------------------------------
    handle.set_feature("ExposureAuto", "Off").await.unwrap();
    let state = handle
        .get_feature_state("ExposureTime")
        .await
        .expect("get_feature_state failed");
    assert!(state.is_implemented);
    assert!(state.is_available);
    assert_eq!(state.access_mode, "RW", "ExposureAuto=Off → RW");

    // --- ExposureTime locked by auto-exposure ------------------------------
    handle
        .set_feature("ExposureAuto", "Continuous")
        .await
        .unwrap();
    let state = handle
        .get_feature_state("ExposureTime")
        .await
        .expect("get_feature_state failed");
    assert!(state.is_implemented);
    assert!(state.is_available);
    assert_eq!(
        state.access_mode, "RO",
        "ExposureAuto=Continuous → pIsLocked downgrades RW to RO"
    );

    // --- AcquisitionFrameRate availability toggled by enable ---------------
    handle
        .set_feature("AcquisitionFrameRateEnable", "0")
        .await
        .unwrap();
    let state = handle
        .get_feature_state("AcquisitionFrameRate")
        .await
        .expect("get_feature_state failed");
    assert!(
        !state.is_available,
        "AcquisitionFrameRateEnable=0 → unavailable"
    );
    assert_eq!(
        state.access_mode, "RO",
        "unavailable feature surfaces as RO"
    );

    handle
        .set_feature("AcquisitionFrameRateEnable", "1")
        .await
        .unwrap();
    let state = handle
        .get_feature_state("AcquisitionFrameRate")
        .await
        .expect("get_feature_state failed");
    assert!(
        state.is_available,
        "AcquisitionFrameRateEnable=1 → available"
    );
    assert_eq!(state.access_mode, "RW");

    // --- PixelFormat entries filtered by SensorType ------------------------
    handle
        .set_feature("SensorType", "Monochrome")
        .await
        .unwrap();
    let state = handle
        .get_feature_state("PixelFormat")
        .await
        .expect("get_feature_state failed");
    let entries = state
        .enum_available
        .expect("PixelFormat enum_available should be populated");
    assert!(entries.contains(&"Mono8".to_string()));
    assert!(entries.contains(&"Mono16".to_string()));
    assert!(!entries.contains(&"BayerRG8".to_string()));
    assert!(!entries.contains(&"RGB8".to_string()));

    handle.set_feature("SensorType", "Color").await.unwrap();
    let state = handle
        .get_feature_state("PixelFormat")
        .await
        .expect("get_feature_state failed");
    let entries = state
        .enum_available
        .expect("PixelFormat enum_available should be populated");
    assert_eq!(entries, vec!["RGB8".to_string()]);
}

/// A feature snapshot of every node the fake declares succeeds, write-only and
/// command nodes included, and none of them is read on the wire (SVC-08,
/// ST-21).
///
/// This is what Studio asks for on connect: the whole graph, in one bulk
/// request. One `WO` node used to fail its own snapshot, so the feature went
/// missing; and a command's backing register was read for a value it does not
/// hold. The fake refuses a read of any `WO` register with ACCESS_DENIED, and
/// its counters show whether a read was even attempted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_snapshot_reports_write_only_and_command_nodes_without_reading_them() {
    let cam = TestCamera::start().await;

    let devices = viva_genicam::gige::discover_all(Duration::from_secs(2))
        .await
        .unwrap();
    let dev_info = devices
        .iter()
        .find(|d| d.ip == Ipv4Addr::LOCALHOST)
        .expect("fake camera not found on loopback");
    let handle = DeviceHandle::connect(dev_info, Some(loopback_iface()))
        .await
        .expect("connect failed");

    // The node list, from the XML the service itself fetched.
    let model = viva_genapi_xml::parse(handle.raw_xml()).expect("parse fake XML");
    let nodemap = viva_genicam::genapi::NodeMap::try_from_xml(model).expect("build nodemap");
    let mut names: Vec<&str> = nodemap.node_names().collect();
    names.sort_unstable();

    let fake = cam.camera.as_ref().expect("fake camera running");
    fake.commands().reset();

    let mut failed = Vec::new();
    let mut states = std::collections::HashMap::new();
    for name in &names {
        match handle.get_feature_state(name).await {
            Ok(state) => {
                states.insert(*name, state);
            }
            Err(e) => failed.push(format!("{name}: {e}")),
        }
    }
    assert!(failed.is_empty(), "snapshot failed for: {failed:#?}");

    // Declared `WO` in the fake's XML: two commands, a command register and a
    // bit of a write-only `<StructReg>`.
    for name in [
        "AcquisitionStart",
        "UserSetLoad",
        "UserSetLoadReg",
        "SoftwareSignal0Pulse",
    ] {
        let state = &states[name];
        assert_eq!(state.access_mode, "WO", "{name}");
        assert!(state.value.is_null(), "{name} has a value: {}", state.value);
    }
    assert_eq!(states["AcquisitionStart"].kind, "Command");
    assert_eq!(states["SoftwareSignal0Pulse"].kind, "Integer");

    // Ordinary features still carry their values.
    assert_eq!(states["Width"].value, serde_json::json!(640));

    for &addr in viva_fake_gige::registers::WRITE_ONLY_REGISTERS {
        for command in [GvcpCommand::ReadReg, GvcpCommand::ReadMem] {
            assert_eq!(
                fake.commands().at(command, addr),
                0,
                "{command:?} of write-only register {addr:#x} reached the wire"
            );
        }
    }
}

/// An event-vision stream is published on `evs`, framed by an `EvsHeader`,
/// and nothing reaches the image topic (DC-05, #138/#139).
///
/// The fake streams EVT 3.0 under the vendor format code the TRT009S-E uses.
/// Before DC-05 each block went out on `image` as a 16 × 4 "image" whose
/// pixel format decoded as `Unknown`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_event_blocks_are_published_on_their_own_topic() {
    use viva_genicam::pfnc::PixelFormat;

    const WIDTH: u32 = 16;
    const HEIGHT: u32 = 4;
    let _cam = TestCamera::start_with(|builder| {
        builder
            .width(WIDTH)
            .height(HEIGHT)
            .pixel_format(PixelFormat::EvsEvt30.code())
    })
    .await;

    let session = Arc::new(zenoh::open(zenoh::Config::default()).await.unwrap());
    let devices = viva_genicam::gige::discover_all(Duration::from_secs(2))
        .await
        .expect("discovery failed");
    let dev_info = devices
        .iter()
        .find(|d| d.ip == Ipv4Addr::LOCALHOST)
        .expect("fake camera not found on loopback");
    let handle = Arc::new(
        DeviceHandle::connect(dev_info, Some(loopback_iface()))
            .await
            .expect("connect failed"),
    );
    let device_id = handle.device_id().to_string();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let tasks = spawn_service_tasks(session.clone(), handle.clone(), shutdown_rx).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let image_sub = session
        .declare_subscriber(keys::image(&device_id))
        .await
        .unwrap();
    let evs_sub = session
        .declare_subscriber(keys::evs(&device_id))
        .await
        .unwrap();

    let resp = send_acq_command(&session, &device_id, AcquisitionCommand::Start).await;
    assert!(resp.ok, "AcquisitionStart failed: {:?}", resp.error);

    let mut seqs = Vec::new();
    for _ in 0..3 {
        let sample = tokio::time::timeout(Duration::from_secs(5), evs_sub.recv_async())
            .await
            .expect("timeout waiting for an event block on the evs topic")
            .expect("subscriber closed");
        let payload = sample.payload().to_bytes();
        let (header, events) = EvsHeader::decode(&payload).expect("EvsHeader decode");
        assert_eq!(header.format, EvsFormat::Evt30);
        assert_eq!(header.payload_len as usize, (WIDTH * HEIGHT) as usize);
        assert_eq!(events.len(), header.payload_len as usize);
        assert!(header.timestamp.is_some(), "the GVSP leader carries one");
        seqs.push(header.seq);
    }
    assert!(
        seqs.windows(2).all(|w| w[1] == w[0].wrapping_add(1)),
        "event block seq must count blocks: {seqs:?}"
    );

    let resp = send_acq_command(&session, &device_id, AcquisitionCommand::Stop).await;
    assert!(resp.ok, "AcquisitionStop failed: {:?}", resp.error);

    // Three blocks arrived on `evs` while `image` was subscribed throughout;
    // an event block published on `image` would be waiting here.
    let stray = tokio::time::timeout(Duration::from_millis(500), image_sub.recv_async()).await;
    assert!(
        stray.is_err(),
        "an event stream published {} bytes on the image topic",
        stray
            .ok()
            .and_then(Result::ok)
            .map_or(0, |s| s.payload().len())
    );

    let _ = shutdown_tx.send(true);
    for task in tasks {
        let _ = tokio::time::timeout(Duration::from_secs(3), task).await;
    }
}
