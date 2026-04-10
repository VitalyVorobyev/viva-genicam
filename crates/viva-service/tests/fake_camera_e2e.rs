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
use viva_service::device::DeviceHandle;
use viva_service::nodes;
use viva_service::status;
use viva_service::xml;

use tokio::sync::{watch, Mutex, OwnedMutexGuard};
use viva_fake_gige::FakeCamera;
use viva_zenoh_api::frame_header::FrameHeader;
use viva_zenoh_api::{keys, AcquisitionCommand, AcquisitionControlRequest, NodeOpResponse};

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
        let guard = camera_lock().lock_owned().await;
        let camera = FakeCamera::builder()
            .bind_ip([127, 0, 0, 1].into())
            .port(3956)
            .width(640)
            .height(480)
            .fps(30)
            .build()
            .await
            .expect("failed to start fake camera");
        TestCamera {
            camera: Some(camera),
            _guard: guard,
        }
    }
}

impl Drop for TestCamera {
    fn drop(&mut self) {
        if let Some(camera) = self.camera.take() {
            camera.stop();
        }
    }
}

fn loopback_iface_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "lo0"
    } else {
        "lo"
    }
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
    nodes::publish_initial_values(&session, &device).await;

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
        tokio::spawn(heartbeat_loop(device.clone(), shutdown.clone())),
    ]
}

async fn heartbeat_loop(device: Arc<DeviceHandle>, mut shutdown: watch::Receiver<bool>) {
    use tokio::time::MissedTickBehavior;

    let mut interval = tokio::time::interval(Duration::from_millis(500));
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = interval.tick() => {
                if device.is_heartbeat_paused() {
                    continue;
                }
                let _ = device.heartbeat_ping().await;
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
            }
        }
    }
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
    let iface_name = loopback_iface_name().to_string();
    let handle = Arc::new(
        DeviceHandle::connect(dev_info, Some(iface_name))
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
        DeviceHandle::connect(dev_info, Some(loopback_iface_name().to_string()))
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
        DeviceHandle::connect(dev_info, Some(loopback_iface_name().to_string()))
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
