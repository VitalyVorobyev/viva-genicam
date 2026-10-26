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

use viva_service::serve_device;
use viva_service::{DeviceHandle, DeviceOps};

use tokio::sync::{Mutex, OwnedMutexGuard, watch};
use viva_fake_gige::{FakeCamera, FakeCameraBuilder, GvcpCommand};
use viva_zenoh_api::frame_header::FrameHeader;
use viva_zenoh_api::{
    AcquisitionCommand, AcquisitionControlRequest, AcquisitionStatus, EvsFormat, EvsHeader,
    NodeOpResponse, keys,
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
fn loopback_iface() -> viva_genicam::gige::Iface {
    viva_genicam::gige::Iface::from_ipv4(std::net::Ipv4Addr::LOCALHOST).expect("loopback iface")
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Send an acquisition control request and return the response.
async fn send_acq_command(
    session: &zenoh::Session,
    device_id: &str,
    command: AcquisitionCommand,
) -> NodeOpResponse {
    let key = keys::acquisition_control(device_id);
    let req = AcquisitionControlRequest::new(command);
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
    let tasks = serve_device(session.clone(), handle.clone(), shutdown_rx).await;

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

/// The acquisition status carries the stream's counters while it runs, and
/// on stop the final counters arrive before the status that says acquisition
/// is off — so a client keeps the numbers after Stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_acquisition_status_carries_the_stream_counters() {
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
    let tasks = serve_device(session.clone(), handle.clone(), shutdown_rx).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let status_sub = session
        .declare_subscriber(keys::acquisition_status(&device_id))
        .await
        .unwrap();
    let next_status = || async {
        let sample = tokio::time::timeout(Duration::from_secs(5), status_sub.recv_async())
            .await
            .expect("no acquisition status within 5 s")
            .expect("subscriber closed");
        serde_json::from_slice::<AcquisitionStatus>(&sample.payload().to_bytes())
            .expect("failed to parse AcquisitionStatus")
    };

    let resp = send_acq_command(&session, &device_id, AcquisitionCommand::Start).await;
    assert!(resp.ok, "AcquisitionStart failed: {:?}", resp.error);

    // The start status has no counters yet; the frame loop's do.
    let counted = loop {
        let status = next_status().await;
        assert!(status.active, "acquisition stopped by itself: {status:?}");
        if let Some(stats) = status.stats.filter(|s| s.frames_complete > 0) {
            assert!(status.fps.is_some(), "a counted status has a rate");
            break stats;
        }
    };
    assert!(counted.packets_received > 0, "{counted:?}");
    assert!(counted.bytes > 0, "{counted:?}");

    let resp = send_acq_command(&session, &device_id, AcquisitionCommand::Stop).await;
    assert!(resp.ok, "AcquisitionStop failed: {:?}", resp.error);

    let mut status = next_status().await;
    while status.active {
        status = next_status().await;
    }
    let last = status
        .stats
        .expect("the first inactive status is the final count");
    assert!(last.frames_complete >= counted.frames_complete, "{last:?}");
    let off = next_status().await;
    assert!(!off.active && off.stats.is_none(), "{off:?}");

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
    let tasks = serve_device(session.clone(), handle.clone(), shutdown_rx).await;
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
    let tasks = serve_device(session.clone(), handle.clone(), shutdown_rx).await;
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

/// A write over Zenoh names the features it may have changed, so a client can
/// re-read the ones it shows (ST-30). On the fake, `ExposureAuto` locks
/// `ExposureTime` through `pIsLocked`, so writing it invalidates
/// `ExposureTime`; writing `Width` does not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_write_reply_names_what_it_invalidated() {
    let _cam = TestCamera::start().await;
    let session = Arc::new(zenoh::open(zenoh::Config::default()).await.unwrap());
    let devices = viva_genicam::gige::discover_all(Duration::from_secs(2))
        .await
        .unwrap();
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
    let tasks = serve_device(session.clone(), handle.clone(), shutdown_rx).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let set = |name: &'static str, value: serde_json::Value| {
        let session = session.clone();
        let key = keys::node_set(&device_id, name);
        async move {
            let payload = serde_json::to_vec(&viva_zenoh_api::NodeSetRequest::new(value)).unwrap();
            let replies = session
                .get(&key)
                .payload(payload)
                .timeout(Duration::from_secs(5))
                .await
                .unwrap();
            let reply = replies.recv_async().await.expect("no reply");
            let sample = reply.result().expect("query error");
            serde_json::from_slice::<NodeOpResponse>(&sample.payload().to_bytes()).unwrap()
        }
    };

    let resp = set("ExposureAuto", serde_json::json!("Continuous")).await;
    assert!(resp.ok, "{:?}", resp.error);
    assert!(
        resp.invalidated.iter().any(|n| n == "ExposureTime"),
        "{:?}",
        resp.invalidated
    );
    let resp = set("Width", serde_json::json!(320)).await;
    assert!(resp.ok, "{:?}", resp.error);
    assert!(
        !resp.invalidated.iter().any(|n| n == "ExposureTime"),
        "{:?}",
        resp.invalidated
    );

    let _ = shutdown_tx.send(true);
    for task in tasks {
        let _ = tokio::time::timeout(Duration::from_secs(3), task).await;
    }
}

/// The service writes the diagnostic report on its camera, for a client that
/// cannot reach the camera itself (ST-41).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_report_describes_the_served_camera() {
    let _cam = TestCamera::start().await;
    let session = Arc::new(zenoh::open(zenoh::Config::default()).await.unwrap());
    let devices = viva_genicam::gige::discover_all(Duration::from_secs(2))
        .await
        .unwrap();
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
    let tasks = serve_device(session.clone(), handle.clone(), shutdown_rx).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let request = viva_zenoh_api::DeviceReportRequest::new(false);
    let replies = session
        .get(keys::report(&device_id))
        .payload(serde_json::to_vec(&request).unwrap())
        .timeout(Duration::from_secs(10))
        .await
        .unwrap();
    let reply = replies.recv_async().await.expect("no reply");
    let sample = reply.result().expect("query error");
    let report = serde_json::from_slice::<viva_zenoh_api::DeviceReportResponse>(
        &sample.payload().to_bytes(),
    )
    .unwrap()
    .report;
    for expected in [
        "tool:        viva-service",
        &format!("device id:   {device_id}"),
        "selected: 127.0.0.1",
        "No features were dropped.",
        "Omitted on request",
    ] {
        assert!(report.contains(expected), "missing {expected:?}:\n{report}");
    }

    let _ = shutdown_tx.send(true);
    for task in tasks {
        let _ = tokio::time::timeout(Duration::from_secs(3), task).await;
    }
}

/// The reserved bulk-state endpoint must not also introspect a feature named
/// `bulk`, even when the client explicitly queries every matching handler.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_bulk_state_has_one_reply_and_single_node_state_still_works() {
    use std::collections::HashMap;
    use viva_zenoh_api::{BulkReadRequest, FeatureState};
    use zenoh::query::{ConsolidationMode, QueryTarget};

    let mut cam = TestCamera::start().await;
    let session = Arc::new(zenoh::open(zenoh::Config::default()).await.unwrap());
    let devices = viva_genicam::gige::discover_all(Duration::from_secs(2))
        .await
        .expect("discovery");
    let info = devices
        .iter()
        .find(|device| device.ip == Ipv4Addr::LOCALHOST)
        .expect("fake camera");
    let handle = Arc::new(
        DeviceHandle::connect(info, Some(loopback_iface()))
            .await
            .expect("connect"),
    );
    let device_id = handle.device_id().to_string();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let tasks = serve_device(session.clone(), handle.clone(), shutdown_rx).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Target all matching queryables and disable consolidation, so an extra
    // reply from `nodes/*/state` cannot be hidden by query routing or merging.
    let bulk_key = keys::nodes_bulk_state(&device_id);
    let replies = session
        .get(&bulk_key)
        .target(QueryTarget::All)
        .consolidation(ConsolidationMode::None)
        .payload(
            serde_json::to_vec(&BulkReadRequest::new(vec!["Width".into(), "Height".into()]))
                .expect("request JSON"),
        )
        .timeout(Duration::from_secs(2))
        .await
        .expect("bulk query");
    let mut bulk_replies = 0;
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Ok(reply) = replies.recv_async().await {
            bulk_replies += 1;
            let sample = reply.result().expect("bulk query must not return errors");
            assert_eq!(sample.key_expr().as_str(), bulk_key);
            let states: HashMap<String, FeatureState> =
                serde_json::from_slice(&sample.payload().to_bytes()).expect("bulk states");
            assert_eq!(states.len(), 2);
            assert_eq!(states["Width"].value, serde_json::json!(640));
            assert_eq!(states["Height"].value, serde_json::json!(480));
        }
    })
    .await
    .expect("bulk replies deadline");
    assert_eq!(bulk_replies, 1, "exactly the bulk handler must reply");

    let replies = session
        .get(keys::node_introspect(&device_id, "Width"))
        .target(QueryTarget::All)
        .consolidation(ConsolidationMode::None)
        .timeout(Duration::from_secs(2))
        .await
        .expect("single-node query");
    let mut single_replies = 0;
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Ok(reply) = replies.recv_async().await {
            single_replies += 1;
            let sample = reply.result().expect("single-node reply");
            let state: FeatureState =
                serde_json::from_slice(&sample.payload().to_bytes()).expect("single state");
            assert_eq!(state.value, serde_json::json!(640));
        }
    })
    .await
    .expect("single-node replies deadline");
    assert_eq!(single_replies, 1);

    shutdown_tx.send(true).expect("shutdown");
    for task in tasks {
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("service shutdown deadline")
            .expect("service task");
    }
    drop(handle);
    drop(session);
    cam.camera.take().expect("running fake").stop().await;
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

    for &addr in viva_fake_gige::WRITE_ONLY_REGISTERS {
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
            .pixel_format(PixelFormat::VendorLucidEvsEvt30.code())
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
    let tasks = serve_device(session.clone(), handle.clone(), shutdown_rx).await;
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

/// A camera whose link went down long enough for its heartbeat to run out
/// comes back with nobody in control (an Allied Vision Manta G-125B did,
/// backlog API-21). The served session is then lost, so `run_discovery`
/// retires it and serves the camera afresh, rather than keeping a session
/// that the camera no longer streams to or takes writes from.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_a_session_the_camera_dropped_is_lost() {
    let cam = TestCamera::start_with(|b| b.enforce_heartbeat(true).heartbeat_timeout_ms(600)).await;
    let fake = cam.camera.as_ref().expect("fake");
    let info = viva_genicam::gige::discover_ip(Ipv4Addr::LOCALHOST, Duration::from_millis(500))
        .await
        .expect("discovery")
        .expect("the fake answers");
    let handle = DeviceHandle::connect(&info, Some(loopback_iface()))
        .await
        .expect("connect");
    assert!(!handle.is_lost());

    fake.unplug();
    tokio::time::sleep(Duration::from_millis(1800)).await;
    assert!(!handle.is_lost(), "silence alone is not a loss");
    fake.plug_in();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !handle.is_lost() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the session was not reported lost"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(fake.controller().await, None);
}

/// A refresh performed before serving replaces all session metadata, not
/// only the camera mutex. In particular a firmware change's XML and a new
/// Discovery ACK belong to the replacement session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_refresh_replaces_identity_xml_control_and_stream() {
    let mut cam = TestCamera::start_with(|b| b.enforce_heartbeat(true)).await;
    let info = viva_genicam::gige::discover_ip(Ipv4Addr::LOCALHOST, Duration::from_millis(500))
        .await
        .expect("discovery")
        .expect("the fake answers");
    let mut handle = DeviceHandle::connect(&info, Some(loopback_iface()))
        .await
        .expect("connect");
    let original_xml = handle.raw_xml().to_string();
    cam.camera.take().expect("old fake").stop().await;
    cam.camera = Some(
        FakeCamera::builder()
            .bind_ip(Ipv4Addr::LOCALHOST)
            .model("ReplacementFirmware")
            .profile(viva_fake_gige::XmlProfile::Legacy)
            .enforce_heartbeat(true)
            .width(320)
            .height(240)
            .build()
            .await
            .expect("replacement fake"),
    );
    handle.refresh_connection().await.expect("refresh");
    assert_eq!(handle.info().model.as_deref(), Some("ReplacementFirmware"));
    assert_ne!(handle.raw_xml(), original_xml);
    assert!(handle.raw_xml().contains("ExposureTimeAbs"));
    assert!(!handle.is_lost());
    handle
        .set_feature("ExposureTimeAbs", "200")
        .await
        .expect("the replacement nodemap is used");
    assert_eq!(
        handle
            .get_feature("ExposureTimeAbs")
            .await
            .expect("exposure"),
        "200"
    );
    let mut frames = viva_service::StreamSource::open_stream(&handle, Some(1500))
        .await
        .expect("open the replacement stream");
    handle.acquisition_start().await.expect("start");
    let frame = tokio::time::timeout(Duration::from_secs(3), frames.next_block())
        .await
        .expect("frame timeout")
        .expect("frame")
        .expect("the stream is open");
    let viva_genicam::StreamBlock::Image(frame) = frame else {
        panic!("expected an image");
    };
    assert_eq!((frame.width, frame.height), (320, 240));
    handle.acquisition_stop().await.expect("stop");
}

/// Retirement stops an active stream and replaces the served XML queryable as
/// well as its camera. A reboot can retain a MAC while changing XML.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_discovery_replaces_a_rebooted_camera_and_resumes_acquisition() {
    let mut cam = TestCamera::start_with(|b| b.enforce_heartbeat(true)).await;
    let session = Arc::new(
        zenoh::open(zenoh::Config::default())
            .await
            .expect("session"),
    );
    let id = viva_zenoh_api::gige_device_id(&viva_fake_gige::FAKE_MAC);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let discovery = viva_service::GigeDiscovery::new(
        Duration::from_millis(100),
        Some(viva_genicam::gige::IfaceSelector::Name(
            loopback_iface().name().to_owned(),
        )),
    );
    let service = tokio::spawn(viva_service::run_discovery(
        session.clone(),
        discovery,
        Duration::from_millis(100),
        shutdown_rx,
    ));

    async fn wait_for_xml(session: &zenoh::Session, id: &str, feature: &str) {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let replies = session
                    .get(keys::xml(id))
                    .timeout(Duration::from_millis(200))
                    .await
                    .expect("XML query");
                while let Ok(reply) = replies.recv_async().await {
                    if let Ok(sample) = reply.result()
                        && let Ok(response) = serde_json::from_slice::<
                            viva_zenoh_api::DeviceXmlResponse,
                        >(&sample.payload().to_bytes())
                        && response.xml.contains(feature)
                    {
                        return;
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the expected XML is served");
    }
    wait_for_xml(&session, &id, "Name=\"ExposureTime\"").await;
    let old_subscriber = session
        .declare_subscriber(keys::image(&id))
        .await
        .expect("old-session subscriber");
    let response = send_acq_command(&session, &id, AcquisitionCommand::Start).await;
    assert!(response.ok, "old-session start: {:?}", response.error);
    let old_frame = tokio::time::timeout(Duration::from_secs(5), old_subscriber.recv_async())
        .await
        .expect("old-session frame deadline")
        .expect("old-session frame");
    let old_payload = old_frame.payload().to_bytes();
    let (header, _) = FrameHeader::decode(&old_payload).expect("old frame header");
    assert_eq!((header.width, header.height), (640, 480));
    drop(old_subscriber);
    // Reboot while streaming: no AcquisitionStop is sent to the old camera.
    // Retirement must release its frame loop before the new session is served.
    cam.camera.take().expect("old fake").stop().await;
    // Let a discovery pass observe the absence before the rebooted camera
    // returns with the same MAC. Its replacement then serves a fresh XML.
    tokio::time::sleep(Duration::from_millis(500)).await;
    cam.camera = Some(
        FakeCamera::builder()
            .bind_ip(Ipv4Addr::LOCALHOST)
            .enforce_heartbeat(true)
            .profile(viva_fake_gige::XmlProfile::Legacy)
            .width(320)
            .height(240)
            .build()
            .await
            .expect("rebooted fake"),
    );
    wait_for_xml(&session, &id, "Name=\"ExposureTimeAbs\"").await;
    let subscriber = session
        .declare_subscriber(keys::image(&id))
        .await
        .expect("subscriber");
    let response = send_acq_command(&session, &id, AcquisitionCommand::Start).await;
    assert!(response.ok, "{:?}", response.error);
    let sample = tokio::time::timeout(Duration::from_secs(5), subscriber.recv_async())
        .await
        .expect("new frame timeout")
        .expect("new frame");
    let payload = sample.payload().to_bytes();
    let (header, _) = FrameHeader::decode(&payload).expect("frame header");
    assert_eq!((header.width, header.height), (320, 240));
    let response = send_acq_command(&session, &id, AcquisitionCommand::Stop).await;
    assert!(response.ok, "{:?}", response.error);
    shutdown_tx.send(true).expect("stop service");
    tokio::time::timeout(Duration::from_secs(15), service)
        .await
        .expect("service shutdown")
        .expect("service task");
    cam.camera.take().expect("rebooted fake").stop().await;
}
