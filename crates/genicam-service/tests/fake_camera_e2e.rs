//! End-to-end integration test: fake camera → genicam-service → Zenoh client.
//!
//! Requires `arv-fake-gv-camera-0.8` on PATH. Run with:
//!
//! ```sh
//! cargo test --test fake_camera_e2e -- --ignored --test-threads=1
//! ```

use std::net::Ipv4Addr;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use genicam_service::acquisition;
use genicam_service::device::DeviceHandle;
use genicam_service::nodes;
use genicam_service::status;
use genicam_service::xml;

use genicam_zenoh_api::frame_header::FrameHeader;
use genicam_zenoh_api::{AcquisitionCommand, AcquisitionControlRequest, NodeOpResponse, keys};
use tokio::sync::watch;

// ---------------------------------------------------------------------------
// Fake camera process guard
// ---------------------------------------------------------------------------

const FAKE_CAMERA_BIN: &str = "arv-fake-gv-camera-0.8";
const GVCP_PORT: u16 = 3956;

struct FakeCamera {
    child: Child,
}

impl FakeCamera {
    fn start() -> Self {
        let child = Command::new(FAKE_CAMERA_BIN)
            .args(["-i", "127.0.0.1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("Failed to start {FAKE_CAMERA_BIN}: {e}"));

        let cam = FakeCamera { child };
        cam.wait_for_ready();
        cam
    }

    fn wait_for_ready(&self) {
        let start = Instant::now();
        let addr: std::net::SocketAddr = ([127, 0, 0, 1], GVCP_PORT).into();
        while start.elapsed() < Duration::from_secs(5) {
            if let Ok(sock) = std::net::UdpSocket::bind("127.0.0.1:0") {
                sock.set_read_timeout(Some(Duration::from_millis(200))).ok();
                let discovery_pkt = [0x42, 0x11, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01];
                if sock.send_to(&discovery_pkt, addr).is_ok() {
                    let mut buf = [0u8; 512];
                    if sock.recv_from(&mut buf).is_ok() {
                        return;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("Fake camera did not start within 5 s");
    }
}

impl Drop for FakeCamera {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn aravis_available() -> bool {
    Command::new(FAKE_CAMERA_BIN)
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
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

/// Spawn the same set of per-device tasks that genicam-service normally runs.
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

/// Full round-trip: discover → connect → stream → receive frames → stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn e2e_acquisition_roundtrip() {
    if !aravis_available() {
        eprintln!("SKIPPED: {FAKE_CAMERA_BIN} not found on PATH");
        return;
    }
    let _cam = FakeCamera::start();

    // 1. Open a Zenoh session.
    let session = Arc::new(zenoh::open(zenoh::Config::default()).await.unwrap());

    // 2. Discover the fake camera on loopback.
    let devices = genicam::gige::discover_all(Duration::from_secs(2))
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
    // It's acceptable to receive a few trailing frames, but eventually it must stop.
    if late.is_ok() {
        // Drain any leftover frames.
        let drain_result =
            tokio::time::timeout(Duration::from_secs(2), subscriber.recv_async()).await;
        // Eventually we should get a timeout.
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
#[ignore]
async fn e2e_double_start_rejected() {
    if !aravis_available() {
        eprintln!("SKIPPED: {FAKE_CAMERA_BIN} not found on PATH");
        return;
    }
    let _cam = FakeCamera::start();

    let session = Arc::new(zenoh::open(zenoh::Config::default()).await.unwrap());
    let devices = genicam::gige::discover_all(Duration::from_secs(2))
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

/// Verify a running acquisition remains live beyond the fake camera heartbeat window.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn e2e_sustained_streaming() {
    if !aravis_available() {
        eprintln!("SKIPPED: {FAKE_CAMERA_BIN} not found on PATH");
        return;
    }
    let _cam = FakeCamera::start();

    let session = Arc::new(zenoh::open(zenoh::Config::default()).await.unwrap());
    let devices = genicam::gige::discover_all(Duration::from_secs(2))
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
